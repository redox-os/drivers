use acpi::aml::object::Object;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SerializableAmlObject {
    String(String),
    Integer(u64),
    Buffer(Vec<u8>),
}
impl SerializableAmlObject {
    pub fn from_object(object: Object) -> Option<Self> {
        match object {
            Object::String(string) => Some(SerializableAmlObject::String(string)),
            Object::Integer(int) => Some(SerializableAmlObject::Integer(int)),
            Object::Buffer(buf) => Some(SerializableAmlObject::Buffer(buf)),
            _ => None,
        }
    }
    pub fn into_object(self) -> Object {
        match self {
            SerializableAmlObject::String(string) => Object::String(string),
            SerializableAmlObject::Integer(int) => Object::Integer(int),
            SerializableAmlObject::Buffer(buf) => Object::Buffer(buf),
        }
    }
}
