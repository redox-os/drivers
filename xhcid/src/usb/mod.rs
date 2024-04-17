pub use self::bos::{bos_capability_descs, BosAnyDevDesc, BosDescriptor, BosSuperSpeedDesc};
pub use self::config::ConfigDescriptor;
pub use self::device::{DeviceDescriptor, DeviceDescriptor8Byte};
pub use self::endpoint::{
    EndpointDescriptor, EndpointTy, HidDescriptor, SuperSpeedCompanionDescriptor,
    SuperSpeedPlusIsochCmpDescriptor, ENDP_ATTR_TY_MASK,
};
pub use self::hub::*;
pub use self::interface::InterfaceDescriptor;
pub use self::setup::{Setup, SetupReq};

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum DescriptorKind {
    None = 0,
    Device = 1,
    Configuration = 2,
    String = 3,
    Interface = 4,
    Endpoint = 5,
    DeviceQualifier = 6,
    OtherSpeedConfiguration = 7,
    InterfacePower = 8,
    OnTheGo = 9,
    BinaryObjectStorage = 15,
    Hid = 33,
    Hub = 41,
    SuperSpeedCompanion = 48,
}

pub(crate) mod bos;
pub(crate) mod config;
pub(crate) mod device;
pub(crate) mod endpoint;
pub(crate) mod hub;
pub(crate) mod interface;
pub(crate) mod setup;
