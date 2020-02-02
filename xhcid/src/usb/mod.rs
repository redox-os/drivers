pub use self::bos::{BosDescriptor, BosAnyDevDesc, BosSuperSpeedDesc, bos_capability_descs};
pub use self::config::ConfigDescriptor;
pub use self::device::DeviceDescriptor;
pub use self::endpoint::{EndpointDescriptor, HidDescriptor, SuperSpeedCompanionDescriptor, EndpointTy, ENDP_ATTR_TY_MASK};
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

pub(crate) mod bos;
pub(crate) mod config;
pub(crate) mod device;
pub(crate) mod endpoint;
pub(crate) mod interface;
pub(crate) mod setup;
