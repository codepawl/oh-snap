//! Text in a picture, read by Vision.

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_foundation::{NSArray, NSDictionary};
use objc2_vision::{VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel};

use super::cgimage::cgimage_from;
use crate::output::Image;

pub fn read_text(image: &Image) -> Result<String, String> {
    let picture = cgimage_from(image)?;
    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    request.setUsesLanguageCorrection(true);
    let options = NSDictionary::new();
    let handler = unsafe { VNImageRequestHandler::initWithCGImage_options(VNImageRequestHandler::alloc(), &picture, &options) };
    let requests: Retained<NSArray<VNRequest>> = NSArray::from_retained_slice(&[Retained::into_super(Retained::into_super(request.clone()))]);
    handler.performRequests_error(&requests).map_err(|error| error.localizedDescription().to_string())?;

    let Some(observations) = request.results() else {
        return Ok(String::new());
    };
    // Vision's y runs from the bottom, so the highest top edge is the first line.
    let mut lines: Vec<(f64, String)> = Vec::new();
    for observation in observations.iter() {
        let box_ = unsafe { observation.boundingBox() };
        let top = box_.origin.y + box_.size.height;
        if let Some(best) = observation.topCandidates(1).iter().next() {
            lines.push((top, best.string().to_string()));
        }
    }
    lines.sort_by(|first, second| second.0.total_cmp(&first.0));
    Ok(lines.into_iter().map(|(_, text)| text).collect::<Vec<_>>().join("\n"))
}

