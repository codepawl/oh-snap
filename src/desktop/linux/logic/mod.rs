//! Pure logic for the Linux platform: parsing, geometry, command lines and file texts. Nothing
//! here touches the OS or `crate::`, so these files and their tests also run on Windows.

pub mod command;
pub mod install;
pub mod media;
pub mod session;
pub mod update;
pub mod uri;
pub mod x11;
