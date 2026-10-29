//! Screen capture through DXGI Desktop Duplication.
//!
//! The capturer keeps one duplication per monitor alive between captures. DXGI only
//! hands out a new frame when the screen changed since the last one we acquired, so
//! the composite buffer is updated in place and stays valid when nothing changed.

use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION, DXGI_MODE_ROTATION_ROTATE90,
    DXGI_MODE_ROTATION_ROTATE180, DXGI_MODE_ROTATION_ROTATE270,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_NOT_FOUND, DXGI_ERROR_WAIT_TIMEOUT,
    DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput1, IDXGIOutput5,
    IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::{Interface, Result};

/// A rectangle in virtual-desktop pixels (physical, not DPI scaled).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PixelRect {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

impl PixelRect {
    pub fn from_edges(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left: left.min(right),
            top: top.min(bottom),
            width: (right - left).abs(),
            height: (bottom - top).abs(),
        }
    }

    pub fn right(&self) -> i32 {
        self.left + self.width
    }

    pub fn bottom(&self) -> i32 {
        self.top + self.height
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right() && y >= self.top && y < self.bottom()
    }

    pub fn intersect(&self, other: &PixelRect) -> Option<PixelRect> {
        let left = self.left.max(other.left);
        let top = self.top.max(other.top);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right <= left || bottom <= top {
            return None;
        }
        Some(PixelRect::from_edges(left, top, right, bottom))
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }
}

/// The whole virtual desktop as one BGRA image, plus where each monitor sits in it.
pub struct Screenshot {
    pub bounds: PixelRect,
    pub monitors: Vec<PixelRect>,
    /// BGRA rows, `bounds.width * 4` bytes each, alpha forced to 255.
    pub pixels: Vec<u8>,
}

impl Screenshot {
    pub fn stride(&self) -> usize {
        self.bounds.width as usize * 4
    }

    pub fn monitor_at(&self, x: i32, y: i32) -> Option<PixelRect> {
        self.monitors.iter().copied().find(|monitor| monitor.contains(x, y))
    }

    pub fn pixel_at(&self, x: i32, y: i32) -> Option<[u8; 3]> {
        if !self.bounds.contains(x, y) {
            return None;
        }
        let column = (x - self.bounds.left) as usize;
        let row = (y - self.bounds.top) as usize;
        let offset = row * self.stride() + column * 4;
        let blue = self.pixels[offset];
        let green = self.pixels[offset + 1];
        let red = self.pixels[offset + 2];
        Some([red, green, blue])
    }
}

struct MonitorDuplication {
    context: ID3D11DeviceContext,
    device: ID3D11Device,
    duplication: IDXGIOutputDuplication,
    /// The latest desktop image, kept in video memory between snips.
    latest_frame: Option<ID3D11Texture2D>,
    desktop_rect: PixelRect,
    rotation: DXGI_MODE_ROTATION,
}

pub struct Capturer {
    monitors: Vec<MonitorDuplication>,
    screenshot: Screenshot,
}

impl Capturer {
    pub fn new() -> Result<Self> {
        let monitors = open_all_monitors()?;
        let bounds = union_of(monitors.iter().map(|monitor| monitor.desktop_rect));
        let monitor_rects = monitors.iter().map(|monitor| monitor.desktop_rect).collect();
        let screenshot = Screenshot {
            bounds,
            monitors: monitor_rects,
            pixels: Vec::new(),
        };
        let mut capturer = Self { monitors, screenshot };
        // Take the first frames now: DXGI hands out a frame only when the screen changes,
        // so a frame not taken here might never come while the screen sits still.
        capturer.update_frames()?;
        Ok(capturer)
    }

    /// Brings the screenshot up to date and returns it. Rebuilds the duplications once
    /// when Windows invalidated them (resolution change, secure desktop, driver reset).
    pub fn capture(&mut self) -> Result<&Screenshot> {
        if let Err(first_error) = self.update_frames() {
            if first_error.code() != DXGI_ERROR_ACCESS_LOST {
                return Err(first_error);
            }
            *self = Capturer::new()?;
        }
        self.read_back()?;
        Ok(&self.screenshot)
    }

    pub fn screenshot(&self) -> &Screenshot {
        &self.screenshot
    }

    /// Frees the CPU copy between snips; the frames stay in video memory.
    pub fn release_cpu_memory(&mut self) {
        self.screenshot.pixels = Vec::new();
    }

    fn update_frames(&mut self) -> Result<()> {
        for monitor in &mut self.monitors {
            monitor.update_frame()?;
        }
        Ok(())
    }

    fn read_back(&mut self) -> Result<()> {
        let bounds = self.screenshot.bounds;
        let byte_count = bounds.width as usize * bounds.height as usize * 4;
        if self.screenshot.pixels.len() != byte_count {
            self.screenshot.pixels = vec![0; byte_count];
        }
        for monitor in &self.monitors {
            monitor.read_back(&mut self.screenshot)?;
        }
        Ok(())
    }
}

impl MonitorDuplication {
    fn update_frame(&mut self) -> Result<()> {
        let timeout_ms = if self.latest_frame.is_some() { 0 } else { 1000 };
        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        let acquired =
            unsafe { self.duplication.AcquireNextFrame(timeout_ms, &mut frame_info, &mut resource) };
        if let Err(error) = acquired {
            // No new frame means the one we hold is still what is on screen.
            if error.code() == DXGI_ERROR_WAIT_TIMEOUT && self.latest_frame.is_some() {
                return Ok(());
            }
            return Err(error);
        }
        let copy_result = self.keep_frame(resource);
        unsafe { self.duplication.ReleaseFrame()? };
        copy_result
    }

    fn keep_frame(&mut self, resource: Option<IDXGIResource>) -> Result<()> {
        let Some(resource) = resource else {
            return Ok(());
        };
        let frame_texture: ID3D11Texture2D = resource.cast()?;
        let mut frame_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { frame_texture.GetDesc(&mut frame_desc) };
        let kept_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: 0,
            CPUAccessFlags: 0,
            MiscFlags: 0,
            ..frame_desc
        };
        let latest = self.texture_like(&self.latest_frame, &kept_desc)?;
        unsafe { self.context.CopyResource(&latest, &frame_texture) };
        self.latest_frame = Some(latest);
        Ok(())
    }

    fn read_back(&self, screenshot: &mut Screenshot) -> Result<()> {
        let Some(latest) = &self.latest_frame else {
            return Ok(());
        };
        let mut latest_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { latest.GetDesc(&mut latest_desc) };
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            ..latest_desc
        };
        // Created per snip and dropped after it, so it holds no memory while idle.
        let staging = self.texture_like(&None, &staging_desc)?;
        unsafe { self.context.CopyResource(&staging, latest) };

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe { self.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))? };
        let source = MappedFrame {
            data: mapped.pData as *const u8,
            row_pitch: mapped.RowPitch as usize,
            width: latest_desc.Width as usize,
            height: latest_desc.Height as usize,
        };
        write_into_screenshot(&source, self.desktop_rect, self.rotation, screenshot);
        unsafe { self.context.Unmap(&staging, 0) };
        Ok(())
    }

    /// Reuses `existing` when it already has the size and usage asked for.
    fn texture_like(&self, existing: &Option<ID3D11Texture2D>, desc: &D3D11_TEXTURE2D_DESC) -> Result<ID3D11Texture2D> {
        if let Some(existing) = existing {
            let mut existing_desc = D3D11_TEXTURE2D_DESC::default();
            unsafe { existing.GetDesc(&mut existing_desc) };
            if existing_desc.Width == desc.Width && existing_desc.Height == desc.Height && existing_desc.Usage == desc.Usage {
                return Ok(existing.clone());
            }
        }
        let mut texture: Option<ID3D11Texture2D> = None;
        unsafe { self.device.CreateTexture2D(desc, None, Some(&mut texture))? };
        Ok(texture.expect("CreateTexture2D returned no texture"))
    }
}

struct MappedFrame {
    data: *const u8,
    row_pitch: usize,
    width: usize,
    height: usize,
}

/// Copies one monitor's frame into its place in the composite, undoing display rotation.
fn write_into_screenshot(
    source: &MappedFrame,
    desktop_rect: PixelRect,
    rotation: DXGI_MODE_ROTATION,
    screenshot: &mut Screenshot,
) {
    let stride = screenshot.stride();
    let origin_x = (desktop_rect.left - screenshot.bounds.left) as usize;
    let origin_y = (desktop_rect.top - screenshot.bounds.top) as usize;
    let out_width = desktop_rect.width as usize;
    let out_height = desktop_rect.height as usize;

    for out_y in 0..out_height {
        let row_start = (origin_y + out_y) * stride + origin_x * 4;
        let out_row = &mut screenshot.pixels[row_start..row_start + out_width * 4];
        if rotation == DXGI_MODE_ROTATION_ROTATE90
            || rotation == DXGI_MODE_ROTATION_ROTATE180
            || rotation == DXGI_MODE_ROTATION_ROTATE270
        {
            for out_x in 0..out_width {
                let (source_x, source_y) = rotated_source(out_x, out_y, source, rotation);
                let source_pixel = unsafe {
                    std::slice::from_raw_parts(source.data.add(source_y * source.row_pitch + source_x * 4), 4)
                };
                out_row[out_x * 4..out_x * 4 + 4].copy_from_slice(source_pixel);
            }
        } else {
            let copy_width = out_width.min(source.width) * 4;
            let source_row =
                unsafe { std::slice::from_raw_parts(source.data.add(out_y * source.row_pitch), copy_width) };
            out_row[..copy_width].copy_from_slice(source_row);
        }
        force_opaque(out_row);
    }
}

fn rotated_source(out_x: usize, out_y: usize, source: &MappedFrame, rotation: DXGI_MODE_ROTATION) -> (usize, usize) {
    if rotation == DXGI_MODE_ROTATION_ROTATE90 {
        (out_y, source.height - 1 - out_x)
    } else if rotation == DXGI_MODE_ROTATION_ROTATE180 {
        (source.width - 1 - out_x, source.height - 1 - out_y)
    } else {
        (source.width - 1 - out_y, out_x)
    }
}

fn force_opaque(row: &mut [u8]) {
    for pixel in row.chunks_exact_mut(4) {
        pixel[3] = 255;
    }
}

fn open_all_monitors() -> Result<Vec<MonitorDuplication>> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let mut monitors = Vec::new();
    let mut adapter_index = 0;
    loop {
        let adapter: IDXGIAdapter1 = match unsafe { factory.EnumAdapters1(adapter_index) } {
            Ok(adapter) => adapter,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => return Err(error),
        };
        adapter_index += 1;
        open_adapter_monitors(&adapter, &mut monitors)?;
    }
    if monitors.is_empty() {
        return Err(windows::core::Error::new(DXGI_ERROR_NOT_FOUND, "no monitor to capture"));
    }
    Ok(monitors)
}

fn open_adapter_monitors(adapter: &IDXGIAdapter1, monitors: &mut Vec<MonitorDuplication>) -> Result<()> {
    let mut device_and_context: Option<(ID3D11Device, ID3D11DeviceContext)> = None;
    let mut output_index = 0;
    loop {
        let output = match unsafe { adapter.EnumOutputs(output_index) } {
            Ok(output) => output,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => return Err(error),
        };
        output_index += 1;
        let output_desc = unsafe { output.GetDesc()? };
        if !output_desc.AttachedToDesktop.as_bool() {
            continue;
        }
        if device_and_context.is_none() {
            device_and_context = Some(create_device(adapter)?);
        }
        let (device, context) = device_and_context.clone().expect("device was just created");
        let duplication = duplicate_output(&output, &device)?;
        let coordinates: RECT = output_desc.DesktopCoordinates;
        monitors.push(MonitorDuplication {
            context,
            device,
            duplication,
            latest_frame: None,
            desktop_rect: PixelRect::from_edges(coordinates.left, coordinates.top, coordinates.right, coordinates.bottom),
            rotation: output_desc.Rotation,
        });
    }
    Ok(())
}

fn create_device(adapter: &IDXGIAdapter1) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?
    };
    Ok((device.expect("no D3D11 device"), context.expect("no D3D11 context")))
}

fn duplicate_output(output: &windows::Win32::Graphics::Dxgi::IDXGIOutput, device: &ID3D11Device) -> Result<IDXGIOutputDuplication> {
    // DuplicateOutput1 asks for 8-bit BGRA, which also gives a readable frame on HDR displays.
    if let Ok(output5) = output.cast::<IDXGIOutput5>() {
        if let Ok(duplication) = unsafe { output5.DuplicateOutput1(device, 0, &[DXGI_FORMAT_B8G8R8A8_UNORM]) } {
            return Ok(duplication);
        }
    }
    let output1: IDXGIOutput1 = output.cast()?;
    unsafe { output1.DuplicateOutput(device) }
}

fn union_of(rects: impl Iterator<Item = PixelRect>) -> PixelRect {
    let mut left = i32::MAX;
    let mut top = i32::MAX;
    let mut right = i32::MIN;
    let mut bottom = i32::MIN;
    for rect in rects {
        left = left.min(rect.left);
        top = top.min(rect.top);
        right = right.max(rect.right());
        bottom = bottom.max(rect.bottom());
    }
    PixelRect::from_edges(left, top, right, bottom)
}
