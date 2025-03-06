extern crate gpt;
extern crate uuid;

pub type Result<T> = std::io::Result<T>;
mod mbr;
pub mod partition;
pub use self::partition::*;
