use acpi::{
    aml::{
        namespace::AmlName,
        object::{
            FieldAccessType, FieldFlags, FieldUnit, FieldUnitKind, FieldUpdateRule, MethodFlags,
            Object, ReferenceKind, WrappedObject,
        },
        op_region::{OpRegion, RegionSpace},
        Interpreter,
    },
    Handle, Handler,
};
use serde::{Deserialize, Serialize};
use std::{
    ops::{Deref, Shl},
    str::FromStr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

#[derive(Debug, Serialize, Deserialize)]
pub struct AmlSerde {
    pub name: String,
    pub value: AmlSerdeValue,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum AmlSerdeValue {
    Uninitialized,
    Integer(u64),
    String(String),
    OpRegion {
        region: AmlSerdeRegionSpace,
        offset: u64,
        length: u64,
        parent_device: String,
    },
    Field {
        kind: AmlSerdeFieldKind,
        flags: AmlSerdeFieldFlags,
        offset: u64,
        length: u64,
    },
    Device,
    Event(u64),
    Method {
        arg_count: usize,
        serialize: bool,
        sync_level: u8,
    },
    Buffer(Vec<u8>),
    BufferField {
        offset: u64,
        length: u64,
        data: Box<AmlSerdeValue>,
    },
    Processor {
        id: u8,
        pblk_address: u32,
        pblk_len: u8,
    },
    Mutex {
        mutex: u32,
        sync_level: u8,
    },
    Reference {
        kind: AmlSerdeReferenceKind,
        inner: Box<AmlSerdeValue>,
    },
    Package {
        contents: Vec<AmlSerdeValue>,
    },
    PowerResource {
        system_level: u8,
        resource_order: u16,
    },
    RawDataBuffer,
    ThermalZone,
    Debug,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum AmlSerdeRegionSpace {
    SystemMemory,
    SystemIo,
    PciConfig,
    EmbeddedControl,
    SMBus,
    SystemCmos,
    PciBarTarget,
    IPMI,
    GeneralPurposeIo,
    GenericSerialBus,
    Pcc,
    OemDefined(u8),
}

#[derive(Debug, Serialize, Deserialize)]
pub enum AmlSerdeFieldKind {
    Normal {
        region: Box<AmlSerdeValue>,
    },
    Bank {
        region: Box<AmlSerdeValue>,
        bank: Box<AmlSerdeValue>,
        bank_value: u64,
    },
    Index {
        index: Box<AmlSerdeValue>,
        data: Box<AmlSerdeValue>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AmlSerdeFieldFlags {
    pub access_type: AmlSerdeFieldAccessType,
    pub lock_rule: bool, // bit 4
    pub update_rule: AmlSerdeFieldUpdateRule,
}
impl Into<u8> for AmlSerdeFieldFlags {
    fn into(self) -> u8 {
        // bits 0..4
        (self.access_type as u8) +
        // bit 4
        (self.lock_rule as u8).shl(4) +
        // bits 5..7
        (self.update_rule as u8).shl(5)
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[repr(u8)]
pub enum AmlSerdeFieldAccessType {
    Any = 0,
    Byte = 1,
    Word = 2,
    DWord = 3,
    QWord = 4,
    Buffer = 5,
}

#[derive(Debug, Serialize, Deserialize)]
#[repr(u8)]
pub enum AmlSerdeFieldUpdateRule {
    Preserve = 0,
    WriteAsOnes = 1,
    WriteAsZeros = 2,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum AmlSerdeReferenceKind {
    RefOf,
    LocalOrArg,
    Unresolved,
}

impl AmlSerde {
    pub fn default() -> Self {
        Self {
            name: "name".to_owned(),
            value: AmlSerdeValue::String(String::default()),
        }
    }

    pub fn from_aml<H: Handler>(aml_context: &Interpreter<H>, aml_name: &AmlName) -> Option<Self> {
        //TODO: why does namespace.get not take a reference to aml_name
        let aml_value = if let Ok(aml_value) = aml_context.namespace.lock().get(aml_name.clone()) {
            aml_value
        } else {
            return None;
        };

        let value = if let Some(value) = AmlSerdeValue::from_aml_value(aml_value.deref()) {
            value
        } else {
            return None;
        };

        Some(AmlSerde {
            name: aml_name.to_string(),
            value,
        })
    }
}

impl AmlSerdeValue {
    pub fn default() -> Self {
        AmlSerdeValue::String("".to_owned())
    }

    pub fn from_aml_value(aml_value: &Object) -> Option<Self> {
        Some(match aml_value {
            Object::Uninitialized => AmlSerdeValue::Uninitialized,
            Object::Integer(n) => AmlSerdeValue::Integer(n.to_owned()),
            Object::String(s) => AmlSerdeValue::String(s.to_owned()),
            Object::OpRegion(region) => AmlSerdeValue::OpRegion {
                region: match region.space {
                    RegionSpace::SystemMemory => AmlSerdeRegionSpace::SystemMemory,
                    RegionSpace::SystemIO => AmlSerdeRegionSpace::SystemIo,
                    RegionSpace::PciConfig => AmlSerdeRegionSpace::PciConfig,
                    RegionSpace::EmbeddedControl => AmlSerdeRegionSpace::EmbeddedControl,
                    RegionSpace::SmBus => AmlSerdeRegionSpace::SMBus,
                    RegionSpace::SystemCmos => AmlSerdeRegionSpace::SystemCmos,
                    RegionSpace::PciBarTarget => AmlSerdeRegionSpace::PciBarTarget,
                    RegionSpace::Ipmi => AmlSerdeRegionSpace::IPMI,
                    RegionSpace::GeneralPurposeIo => AmlSerdeRegionSpace::GeneralPurposeIo,
                    RegionSpace::GenericSerialBus => AmlSerdeRegionSpace::GenericSerialBus,
                    RegionSpace::Pcc => AmlSerdeRegionSpace::Pcc,
                    RegionSpace::Oem(n) => AmlSerdeRegionSpace::OemDefined(n.to_owned()),
                },
                offset: region.base,
                length: region.length,
                parent_device: region.parent_device_path.to_string(),
            },
            Object::FieldUnit(field) => AmlSerdeValue::Field {
                kind: match &field.kind {
                    FieldUnitKind::Normal { region } => AmlSerdeFieldKind::Normal {
                        region: AmlSerdeValue::from_aml_value(region.deref()).map(Box::new)?,
                    },
                    FieldUnitKind::Bank {
                        region,
                        bank,
                        bank_value,
                    } => AmlSerdeFieldKind::Bank {
                        region: AmlSerdeValue::from_aml_value(region.deref()).map(Box::new)?,
                        bank: AmlSerdeValue::from_aml_value(bank.deref()).map(Box::new)?,
                        bank_value: bank_value.to_owned(),
                    },
                    FieldUnitKind::Index { index, data } => AmlSerdeFieldKind::Index {
                        index: AmlSerdeValue::from_aml_value(index.deref()).map(Box::new)?,
                        data: AmlSerdeValue::from_aml_value(data.deref()).map(Box::new)?,
                    },
                },
                flags: AmlSerdeFieldFlags {
                    access_type: match field.flags.access_type() {
                        Ok(FieldAccessType::Any) => AmlSerdeFieldAccessType::Any,
                        Ok(FieldAccessType::Byte) => AmlSerdeFieldAccessType::Byte,
                        Ok(FieldAccessType::Word) => AmlSerdeFieldAccessType::Word,
                        Ok(FieldAccessType::DWord) => AmlSerdeFieldAccessType::DWord,
                        Ok(FieldAccessType::QWord) => AmlSerdeFieldAccessType::QWord,
                        Ok(FieldAccessType::Buffer) => AmlSerdeFieldAccessType::Buffer,
                        _ => return None,
                    },
                    lock_rule: field.flags.lock_rule(),
                    update_rule: match field.flags.update_rule() {
                        FieldUpdateRule::Preserve => AmlSerdeFieldUpdateRule::Preserve,
                        FieldUpdateRule::WriteAsOnes => AmlSerdeFieldUpdateRule::WriteAsOnes,
                        FieldUpdateRule::WriteAsZeros => AmlSerdeFieldUpdateRule::WriteAsZeros,
                    },
                },
                offset: field.bit_index as u64,
                length: field.bit_length as u64,
            },
            Object::Device => AmlSerdeValue::Device,
            Object::Event(event) => AmlSerdeValue::Event(event.load(Ordering::Relaxed)),
            Object::Method { flags, code: _ } => AmlSerdeValue::Method {
                arg_count: flags.arg_count(),
                serialize: flags.serialize(),
                sync_level: flags.sync_level(),
            },
            //TODO: distinguish from Method?
            Object::NativeMethod { f: _, flags } => AmlSerdeValue::Method {
                arg_count: flags.arg_count(),
                serialize: flags.serialize(),
                sync_level: flags.sync_level(),
            },
            Object::Buffer(buffer_data) => AmlSerdeValue::Buffer(buffer_data.to_owned()),
            Object::BufferField {
                buffer,
                offset,
                length,
            } => AmlSerdeValue::BufferField {
                offset: offset.to_owned() as u64,
                length: length.to_owned() as u64,
                data: AmlSerdeValue::from_aml_value(buffer.deref()).map(Box::new)?,
            },
            Object::Processor {
                proc_id,
                pblk_address,
                pblk_length,
            } => AmlSerdeValue::Processor {
                id: proc_id.to_owned(),
                pblk_address: pblk_address.to_owned(),
                pblk_len: pblk_length.to_owned(),
            },
            Object::Mutex { mutex, sync_level } => AmlSerdeValue::Mutex {
                mutex: mutex.0,
                sync_level: sync_level.to_owned(),
            },
            Object::Reference { kind, inner } => AmlSerdeValue::Reference {
                kind: match kind {
                    ReferenceKind::RefOf => AmlSerdeReferenceKind::RefOf,
                    ReferenceKind::LocalOrArg => AmlSerdeReferenceKind::LocalOrArg,
                    ReferenceKind::Unresolved => AmlSerdeReferenceKind::Unresolved,
                },
                inner: AmlSerdeValue::from_aml_value(inner.deref()).map(Box::new)?,
            },
            Object::Package(aml_contents) => AmlSerdeValue::Package {
                contents: aml_contents
                    .iter()
                    .filter_map(|item| AmlSerdeValue::from_aml_value(item))
                    .collect(),
            },
            Object::PowerResource {
                system_level,
                resource_order,
            } => AmlSerdeValue::PowerResource {
                system_level: system_level.to_owned(),
                resource_order: resource_order.to_owned(),
            },
            Object::RawDataBuffer => AmlSerdeValue::RawDataBuffer,
            Object::ThermalZone => AmlSerdeValue::ThermalZone,
            Object::Debug => AmlSerdeValue::Debug,
        })
    }
    pub fn to_aml_object(self) -> Option<Object> {
        Some(match self {
            AmlSerdeValue::Uninitialized => Object::Uninitialized,
            AmlSerdeValue::Integer(n) => Object::Integer(n),
            AmlSerdeValue::String(s) => Object::String(s),
            AmlSerdeValue::OpRegion {
                region,
                offset,
                length,
                parent_device,
            } => Object::OpRegion(OpRegion {
                space: match region {
                    AmlSerdeRegionSpace::PciConfig => RegionSpace::PciConfig,
                    AmlSerdeRegionSpace::EmbeddedControl => RegionSpace::EmbeddedControl,
                    AmlSerdeRegionSpace::SMBus => RegionSpace::SmBus,
                    AmlSerdeRegionSpace::SystemCmos => RegionSpace::SystemCmos,
                    AmlSerdeRegionSpace::PciBarTarget => RegionSpace::PciBarTarget,
                    AmlSerdeRegionSpace::IPMI => RegionSpace::Ipmi,
                    AmlSerdeRegionSpace::GeneralPurposeIo => RegionSpace::GeneralPurposeIo,
                    AmlSerdeRegionSpace::GenericSerialBus => RegionSpace::GenericSerialBus,
                    AmlSerdeRegionSpace::SystemMemory => RegionSpace::SystemMemory,
                    AmlSerdeRegionSpace::SystemIo => RegionSpace::SystemIO,
                    AmlSerdeRegionSpace::Pcc => RegionSpace::Pcc,
                    AmlSerdeRegionSpace::OemDefined(n) => RegionSpace::Oem(n),
                },
                base: offset,
                length,
                //
                parent_device_path: AmlName::from_str(&parent_device).ok()?, // TODO: Error value hidden
            }),
            AmlSerdeValue::Field {
                kind,
                flags,
                offset,
                length,
            } => Object::FieldUnit(FieldUnit {
                kind: match kind {
                    AmlSerdeFieldKind::Normal { region } => FieldUnitKind::Normal {
                        region: region.to_aml_object()?.wrap(),
                    },
                    AmlSerdeFieldKind::Bank {
                        region,
                        bank,
                        bank_value,
                    } => FieldUnitKind::Bank {
                        region: region.to_aml_object()?.wrap(),
                        bank: bank.to_aml_object()?.wrap(),
                        bank_value: bank_value.to_owned(),
                    },
                    AmlSerdeFieldKind::Index { index, data } => FieldUnitKind::Index {
                        index: index.to_aml_object()?.wrap(),
                        data: data.to_aml_object()?.wrap(),
                    },
                },
                flags: FieldFlags(flags.into()),
                bit_index: offset as usize,
                bit_length: length as usize,
            }),
            AmlSerdeValue::Device => Object::Device,
            AmlSerdeValue::Event(event) => Object::Event(Arc::new(AtomicU64::new(event))),
            AmlSerdeValue::Method {
                arg_count,
                serialize,
                sync_level,
            } => Object::Method {
                code: (return None), //TODO figure out what to do here
                //TODO check specs to see if all bit patterns are allowed
                flags: MethodFlags(
                    (arg_count as u8).clamp(0, 7)
                        + (serialize as u8).shl(3)
                        + sync_level.clamp(0, 15).shl(4),
                ),
            },
            //TODO: handle native method?
            AmlSerdeValue::Buffer(buffer_data) => Object::Buffer(buffer_data),
            AmlSerdeValue::BufferField {
                data,
                offset,
                length,
            } => Object::BufferField {
                offset: offset as usize,
                length: length as usize,
                buffer: data.to_aml_object()?.wrap(),
            },
            AmlSerdeValue::Processor {
                id,
                pblk_address,
                pblk_len,
            } => Object::Processor {
                proc_id: id,
                pblk_address,
                pblk_length: pblk_len,
            },
            AmlSerdeValue::Mutex { mutex, sync_level } => Object::Mutex {
                mutex: Handle(mutex),
                sync_level: sync_level,
            },
            AmlSerdeValue::Reference { kind, inner } => Object::Reference {
                kind: match kind {
                    AmlSerdeReferenceKind::RefOf => ReferenceKind::RefOf,
                    AmlSerdeReferenceKind::LocalOrArg => ReferenceKind::LocalOrArg,
                    AmlSerdeReferenceKind::Unresolved => ReferenceKind::Unresolved,
                },
                inner: inner.to_aml_object()?.wrap(),
            },
            AmlSerdeValue::Package { contents } => Object::Package(
                contents
                    .into_iter()
                    .map(|item| item.to_aml_object().map(Object::wrap)) // TODO: see if errors should be ignored here
                    .collect::<Option<Vec<WrappedObject>>>()?,
            ),
            AmlSerdeValue::PowerResource {
                system_level,
                resource_order,
            } => Object::PowerResource {
                system_level: system_level.to_owned(),
                resource_order: resource_order.to_owned(),
            },
            AmlSerdeValue::RawDataBuffer => Object::RawDataBuffer,
            AmlSerdeValue::ThermalZone => Object::ThermalZone,
            AmlSerdeValue::Debug => Object::Debug,
        })
    }
}

pub mod aml_serde_name {
    use acpi::aml::namespace::AmlName;

    /// Add a leading backslash to make the name a valid
    /// namespace reference
    pub fn to_aml_format(pretty_name: &String) -> String {
        format!("\\{}", pretty_name)
    }

    /// convert a string from AML namespace style to
    /// acpi symbol style
    pub fn to_symbol(aml_style_name: &String) -> String {
        let mut name = aml_style_name.to_owned();

        // remove leading slash
        name = name.trim_start_matches("\\").to_owned();
        // remove unnecessary underscores
        while let Some(index) = name.find("_.") {
            name.remove(index);
        }
        while name.len() > 0 && &name[name.len() - 1..] == "_" {
            name.pop();
        }
        name.shrink_to_fit();
        name
    }

    /// Convert to string and remove
    /// trailing underscores from each name segment
    pub fn aml_to_symbol(aml_name: &AmlName) -> String {
        to_symbol(&aml_name.as_string())
    }
}
