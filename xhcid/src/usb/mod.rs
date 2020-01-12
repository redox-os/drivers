pub use self::bos::{BosDescriptor, BosAnyDevDesc, BosSuperSpeedDesc, bos_capability_descs};
pub use self::config::ConfigDescriptor;
pub use self::device::DeviceDescriptor;
pub use self::endpoint::{EndpointDescriptor, SuperSpeedCompanionDescriptor};
pub use self::interface::InterfaceDescriptor;
pub use self::setup::Setup;

#[repr(u8)]
pub enum DescriptorKind {
    None,
    Device,
    Configuration,
    String,
    Interface,
    Endpoint,
    DeviceQualifier,
    OtherSpeedConfiguration,
    InterfacePower,
    OnTheGo,
    BinaryObjectStorage = 15,
    SuperSpeedCompanion = 48,
}

mod bos;
mod config;
mod device;
mod endpoint;
mod interface;
mod setup;
