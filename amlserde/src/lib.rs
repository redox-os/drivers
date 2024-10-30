use aml::value::{FieldAccessType, FieldUpdateRule, RegionSpace};
use aml::{AmlContext, AmlHandle, AmlName, AmlValue};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct AmlSerde {
    pub name: String,
    pub value: AmlSerdeValue,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum AmlSerdeValue {
    Boolean(bool),
    Integer(u64),
    String(String),
    OpRegion {
        region: AmlSerdeRegionSpace,
        offset: u64,
        length: u64,
        parent_device: Option<String>,
    },
    Field {
        region: String,
        flags: AmlSerdeFieldFlags,
        offset: u64,
        length: u64,
    },
    Device,
    Method {
        arg_count: u8,
        serialize: bool,
        sync_level: u8,
    },
    Buffer(Vec<u8>),
    BufferField {
        offset: u64,
        length: u64,
        data: Vec<u8>,
    },
    Processor {
        id: u8,
        pblk_address: u32,
        pblk_len: u8,
    },
    Mutex {
        sync_level: u8,
    },
    Package {
        contents: Vec<AmlSerdeValue>,
    },
    PowerResource {
        system_level: u8,
        resource_order: u16,
    },
    ThermalZone,
    External,
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
    OemDefined(u8),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AmlSerdeFieldFlags {
    pub access_type: AmlSerdeFieldAccessType,
    pub lock_rule: bool,
    pub update_rule: AmlSerdeFieldUpdateRule,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum AmlSerdeFieldAccessType {
    Any,
    Byte,
    Word,
    DWord,
    QWord,
    Buffer,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum AmlSerdeFieldUpdateRule {
    Preserve,
    WriteAsOnes,
    WriteAsZeros,
}

impl AmlSerde {
    pub fn default() -> Self {
        Self {
            name: "name".to_owned(),
            value: AmlSerdeValue::String(String::default()),
        }
    }

    pub fn from_aml(
        aml_context: &mut AmlContext,
        aml_lookup: &AmlHandleLookup,
        name: &String,
        aml_name: &AmlName,
        handle: &AmlHandle,
    ) -> Option<Self> {
        let aml_value = if let Ok(aml_value) = aml_context.namespace.get(handle.clone()) {
            aml_value
        } else {
            return None;
        };

        let value = if let Some(value) = AmlSerdeValue::from_aml_value(aml_value, aml_lookup) {
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

    fn from_aml_value(aml_value: &AmlValue, aml_lookup: &AmlHandleLookup) -> Option<Self> {
        Some(match aml_value {
            AmlValue::Boolean(b) => AmlSerdeValue::Boolean(b.to_owned()),

            AmlValue::Integer(n) => AmlSerdeValue::Integer(n.to_owned()),

            AmlValue::String(s) => AmlSerdeValue::String(s.to_owned()),

            AmlValue::OpRegion {
                region,
                offset,
                length,
                parent_device,
            } => AmlSerdeValue::OpRegion {
                region: match region {
                    RegionSpace::SystemMemory => AmlSerdeRegionSpace::SystemMemory,
                    RegionSpace::SystemIo => AmlSerdeRegionSpace::SystemIo,
                    RegionSpace::PciConfig => AmlSerdeRegionSpace::PciConfig,
                    RegionSpace::EmbeddedControl => AmlSerdeRegionSpace::EmbeddedControl,
                    RegionSpace::SMBus => AmlSerdeRegionSpace::SMBus,
                    RegionSpace::SystemCmos => AmlSerdeRegionSpace::SystemCmos,
                    RegionSpace::PciBarTarget => AmlSerdeRegionSpace::PciBarTarget,
                    RegionSpace::IPMI => AmlSerdeRegionSpace::IPMI,
                    RegionSpace::GeneralPurposeIo => AmlSerdeRegionSpace::GeneralPurposeIo,
                    RegionSpace::GenericSerialBus => AmlSerdeRegionSpace::GenericSerialBus,
                    RegionSpace::OemDefined(n) => AmlSerdeRegionSpace::OemDefined(n.to_owned()),
                },
                offset: offset.to_owned(),
                length: length.to_owned(),
                parent_device: if let Some(parent) = parent_device {
                    Some(parent.to_string())
                } else {
                    None
                },
            },

            AmlValue::Field {
                region,
                flags,
                offset,
                length,
            } => AmlSerdeValue::Field {
                region: if let Some((region, _handle)) = aml_lookup.get(region) {
                    region.to_string()
                } else {
                    return None;
                },
                flags: AmlSerdeFieldFlags {
                    access_type: match flags.access_type() {
                        Ok(FieldAccessType::Any) => AmlSerdeFieldAccessType::Any,
                        Ok(FieldAccessType::Byte) => AmlSerdeFieldAccessType::Byte,
                        Ok(FieldAccessType::Word) => AmlSerdeFieldAccessType::Word,
                        Ok(FieldAccessType::DWord) => AmlSerdeFieldAccessType::DWord,
                        Ok(FieldAccessType::QWord) => AmlSerdeFieldAccessType::QWord,
                        Ok(FieldAccessType::Buffer) => AmlSerdeFieldAccessType::Buffer,
                        _ => return None,
                    },
                    lock_rule: flags.lock_rule(),
                    update_rule: match flags.field_update_rule() {
                        Ok(FieldUpdateRule::Preserve) => AmlSerdeFieldUpdateRule::Preserve,
                        Ok(FieldUpdateRule::WriteAsOnes) => AmlSerdeFieldUpdateRule::WriteAsOnes,
                        Ok(FieldUpdateRule::WriteAsZeros) => AmlSerdeFieldUpdateRule::WriteAsZeros,
                        _ => return None,
                    },
                },
                offset: offset.to_owned(),
                length: length.to_owned(),
            },

            AmlValue::Device => AmlSerdeValue::Device,

            AmlValue::Method { flags, code: _ } => AmlSerdeValue::Method {
                arg_count: flags.arg_count(),
                serialize: flags.serialize(),
                sync_level: flags.sync_level(),
            },
            AmlValue::Buffer(buffer_data) => AmlSerdeValue::Buffer({ buffer_data.lock().to_owned() }),
            AmlValue::BufferField {
                buffer_data,
                offset,
                length,
            } => AmlSerdeValue::BufferField {
                offset: offset.to_owned(),
                length: length.to_owned(),
                data: { buffer_data.lock().to_owned() }
            },
            AmlValue::Processor {
                id,
                pblk_address,
                pblk_len,
            } => AmlSerdeValue::Processor {
                id: id.to_owned(),
                pblk_address: pblk_address.to_owned(),
                pblk_len: pblk_len.to_owned(),
            },
            AmlValue::Mutex { sync_level } => AmlSerdeValue::Mutex {
                sync_level: sync_level.to_owned(),
            },
            AmlValue::Package(aml_contents) => AmlSerdeValue::Package {
                contents: aml_contents
                    .iter()
                    .filter_map(|item| AmlSerdeValue::from_aml_value(item, aml_lookup))
                    .collect(),
            },

            AmlValue::PowerResource {
                system_level,
                resource_order,
            } => AmlSerdeValue::PowerResource {
                system_level: system_level.to_owned(),
                resource_order: resource_order.to_owned(),
            },
            AmlValue::ThermalZone => AmlSerdeValue::ThermalZone,
            AmlValue::External => AmlSerdeValue::External,
        })
    }
}

pub mod aml_serde_name {
    use aml::AmlName;

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

pub struct AmlHandleLookup {
    map: FxHashMap<String, (AmlName, AmlHandle)>,
}

impl AmlHandleLookup {
    pub fn new() -> Self {
        Self {
            map: FxHashMap::default(),
        }
    }

    fn handle_to_key(&self, handle: &AmlHandle) -> String {
        format!("{:?}", handle)
    }

    pub fn insert(&mut self, handle: AmlHandle, aml_name: AmlName) {
        self.map
            .insert(self.handle_to_key(&handle), (aml_name, handle));
    }

    pub fn get(&self, handle: &AmlHandle) -> Option<&(AmlName, AmlHandle)> {
        self.map.get(&self.handle_to_key(handle))
    }
}
