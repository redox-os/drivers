pub use self::config::ConfigDescriptor;
pub use self::device::DeviceDescriptor;
pub use self::endpoint::EndpointDescriptor;
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
}

mod config;
mod device;
mod endpoint;
mod interface;
mod setup;
