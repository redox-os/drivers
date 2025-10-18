use acpi::aml::object::{Object, WrappedObject};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct AmlMethodArgs(Vec<AmlObject>);
impl AmlMethodArgs {
    pub fn into_objects(self) -> Vec<WrappedObject> {
        self.0
            .into_iter()
            .map(|arg_kind| WrappedObject::new(arg_kind.into_object()))
            .collect()
    }
    pub fn from_objects(objects: Vec<AmlObject>) -> Self {
        Self(objects)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub enum AmlObject {
    String(String),
    Integer(u64),
    Buffer(Vec<u8>),
}
impl AmlObject {
    pub fn from_object(object: Object) -> Option<Self> {
        match object {
            Object::String(string) => Some(AmlObject::String(string)),
            Object::Integer(int) => Some(AmlObject::Integer(int)),
            Object::Buffer(buf) => Some(AmlObject::Buffer(buf)),
            _ => None,
        }
    }
    pub fn into_object(self) -> Object {
        match self {
            AmlObject::String(string) => Object::String(string),
            AmlObject::Integer(int) => Object::Integer(int),
            AmlObject::Buffer(buf) => Object::Buffer(buf),
        }
    }
}
