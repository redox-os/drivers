use std::{cmp, mem, str};
use std::convert::TryFrom;
use std::io::prelude::*;

use plain::Plain;
use serde::Serialize;
use smallvec::SmallVec;

use syscall::io::Io;
use syscall::scheme::SchemeMut;
use syscall::{
    EACCES, EBADF, EINVAL, EISDIR, ENOENT,
    O_CREAT, O_DIRECTORY, O_STAT, ENOSYS, ENOTDIR,
    MODE_DIR, MODE_FILE,
    SEEK_CUR, SEEK_END, SEEK_SET,
    Stat,
    Error, Result
};

use super::{Device, Xhci};
use super::{port, usb};

pub enum Handle {
    TopLevel(usize, Vec<u8>), // offset, contents (ports)
    Port(usize, usize, Vec<u8>), // port, offset, contents
    PortDesc(usize, usize, Vec<u8>), // port, offset, contents
    Endpoints(usize, usize, Vec<u8>), // port, offset, contents
    Endpoint(usize, usize, usize), // port, endpoint, offset
}

#[derive(Serialize)]
struct PortDesc {
    // I have no idea whether this number is useful to the API users.
    slot: u8,
    dev_desc: DevDescJson,
}

#[derive(Serialize)]
struct DevDescJson {
    // TODO: length?
    kind: u8,
    usb: u16,
    class: u8,
    sub_class: u8,
    protocol: u8,
    packet_size: u8,
    vendor: u16,
    product: u16,
    release: u16,
    manufacturer_str: Option<String>,
    product_str: Option<String>,
    serial_str: Option<String>,
    config_descs: SmallVec<[ConfDescJson; 1]>,
}

#[derive(Serialize)]
struct ConfDescJson {
    // TODO: length?
    kind: u8,
    // TODO: total_length?
    // TODO: configuration_value?
    configuration: Option<String>,
    attributes: u8,
    max_power: u8,
    interface_descs: SmallVec<[IfDescJson; 1]>,
}

#[derive(Serialize)]
struct EndpDescJson {
    kind: u8,
    address: u8,
    attributes: u8,
    max_packet_size: u16,
    interval: u8,
}
impl From<usb::EndpointDescriptor> for EndpDescJson {
    fn from(d: usb::EndpointDescriptor) -> Self {
        Self {
            kind: d.kind,
            address: d.address,
            attributes: d.attributes,
            interval: d.interval,
            max_packet_size: d.max_packet_size,
        }
    }
}

#[derive(Serialize)]
struct IfDescJson {
    // TODO: length?
    kind: u8,
    number: u8,
    alternate_setting: u8,
    class: u8,
    sub_class: u8,
    protocol: u8,
    interface_str: Option<String>,
    endpoints: SmallVec<[AnyEndpDescJson; 4]>,
}
impl IfDescJson {
    fn new(dev: &mut Device, desc: usb::InterfaceDescriptor, endps: impl IntoIterator<Item = AnyEndpDescJson>) -> Result<Self> {
        Ok(Self {
            alternate_setting: desc.alternate_setting,
            class: desc.class,
            interface_str: if desc.interface_str > 0 { Some(dev.get_string(desc.interface_str)?) } else { None },
            kind: desc.kind,
            number: desc.number,
            protocol: desc.protocol,
            sub_class: desc.sub_class,
            endpoints: endps.into_iter().collect(),
        })
    }
}

#[derive(Serialize)]
struct SuperSpeedCmpJson {
    kind: u8,
    max_burst: u8,
    attributes: u8,
    bytes_per_interval: u16,
}

impl From<usb::SuperSpeedCompanionDescriptor> for SuperSpeedCmpJson {
    fn from(d: usb::SuperSpeedCompanionDescriptor) -> Self {
        Self {
            kind: d.kind,
            attributes: d.attributes,
            bytes_per_interval: d.bytes_per_interval,
            max_burst: d.max_burst,
        }
    }
}

#[derive(Serialize)]
enum AnyEndpDescJson {
    Endp(EndpDescJson),
    SuperSpeedCmp(SuperSpeedCmpJson),
}

/// Any descriptor that can be stored in the config desc "data" area.
#[derive(Debug)]
enum AnyDescriptor {
    // These are the ones that I have found, but there are more.
    Device(usb::DeviceDescriptor),
    Config(usb::ConfigDescriptor),
    Interface(usb::InterfaceDescriptor),
    Endpoint(usb::EndpointDescriptor),
    SuperSpeedCompanion(usb::SuperSpeedCompanionDescriptor),
}

impl AnyDescriptor {
    fn parse(bytes: &[u8]) -> Option<(Self, usize)> {
        // There has to be at least two bytes for the kind and length.
        if bytes.len() < 2 { return None }

        let len = bytes[0];
        let kind = bytes[1];

        if bytes.len() < len.into() { return None }

        Some((match kind {
            1 => Self::Device(*plain::from_bytes(bytes).ok()?),
            2 => Self::Config(*plain::from_bytes(bytes).ok()?),
            4 => Self::Interface(*plain::from_bytes(bytes).ok()?),
            5 => Self::Endpoint(*plain::from_bytes(bytes).ok()?),
            48 => Self::SuperSpeedCompanion(*plain::from_bytes(bytes).ok()?),
            _ => {
                //println!("Descriptor unknown {}: bytes {:#0x?}", kind, bytes);
                return None;
            }
        }, len.into()))
    }
}

impl Xhci {
    fn write_port_desc(&mut self, port_id: usize, contents: &mut Vec<u8>) -> Result<()> {
        let port = self.ports.get(port_id).ok_or(Error::new(ENOENT))?;
        if !port.flags().contains(port::PortFlags::PORT_CCS) {
            return Err(Error::new(ENOENT));
        }

        let st = self.port_states.get_mut(&port_id).unwrap();
        
        // TODO: Should the descriptors be stored in PortState?

        self.run.ints[0].erdp.write(self.cmd.erdp());

        let mut dev = Device {
            ring: &mut st.ring,
            cmd: &mut self.cmd,
            db: &mut self.dbs[st.slot as usize],
            int: &mut self.run.ints[0],
        };

        let raw_dd = dev.get_device()?;

        let (manufacturer_str, product_str, serial_str) = (
            if raw_dd.manufacturer_str > 0 {
                Some(dev.get_string(raw_dd.manufacturer_str)?)
            } else { None },
            if raw_dd.product_str > 0 {
                Some(dev.get_string(raw_dd.product_str)?)
            } else { None },
            if raw_dd.serial_str > 0 {
                Some(dev.get_string(raw_dd.serial_str)?)
            } else { None },
        );

        let (bos_desc, bos_data) = dev.get_bos()?;
        writeln!(contents, "BOS BASE {:?}", bos_desc).unwrap();

        let has_superspeed = usb::bos_capability_descs(bos_desc, &bos_data).inspect(|item| println!("{:?}", item)).any(|desc| desc.is_superspeed());

        let config_descs = (0..raw_dd.configurations).map(|index| -> Result<_> {
            // TODO: Actually, it seems like all descriptors contain a length field, and I
            // encountered a SuperSpeed descriptor when endpoints were expected. The right way
            // would probably be to have an enum of all possible descs, and sort them based on
            // location, even though they might not necessarily be ordered trivially.

            let (desc, data) = dev.get_config(index)?;

            let extra_length = desc.total_length as usize - mem::size_of_val(&desc);
            let data = &data[..extra_length];

            let mut i = 0;
            let mut descriptors = Vec::new();

            while let Some((descriptor, len)) = AnyDescriptor::parse(&data[i..]) {
                descriptors.push(descriptor);
                i += len;
            }

            let mut interface_descs = SmallVec::new();
            let mut iter = descriptors.into_iter();

            while let Some(item) = iter.next() {
                if let AnyDescriptor::Interface(idesc) = item {
                    let mut endpoints = SmallVec::<[AnyEndpDescJson; 4]>::new();

                    for _ in 0..idesc.endpoints {
                        let next = match iter.next() {
                            Some(AnyDescriptor::Endpoint(n)) => n,
                            _ => break,
                        };
                        endpoints.push(AnyEndpDescJson::Endp(EndpDescJson::from(next)));

                        if has_superspeed {
                            dbg!();
                            let next = match iter.next() {
                                Some(AnyDescriptor::SuperSpeedCompanion(n)) => n,
                                _ => break,
                            };
                            dbg!();
                            endpoints.push(AnyEndpDescJson::SuperSpeedCmp(SuperSpeedCmpJson::from(next)));
                        }
                    }

                    interface_descs.push(IfDescJson::new(&mut dev, idesc, endpoints)?);
                } else {
                    // TODO
                    break;
                }
            }

            Ok(ConfDescJson {
                kind: desc.kind,
                configuration: if desc.configuration_str > 0 { Some(dev.get_string(desc.configuration_str)?) } else { None },
                attributes: desc.attributes,
                max_power: desc.max_power,
                interface_descs,
            })
        }).collect::<Result<SmallVec<_>>>()?;

        let dev_desc = DevDescJson {
            kind: raw_dd.kind,
            usb: raw_dd.usb,
            class: raw_dd.class,
            sub_class: raw_dd.sub_class,
            protocol: raw_dd.protocol,
            packet_size: raw_dd.packet_size,
            vendor: raw_dd.vendor,
            product: raw_dd.product,
            release: raw_dd.release,
            manufacturer_str,
            product_str,
            serial_str,
            config_descs,
        };

        let desc = PortDesc {
            slot: st.slot,
            dev_desc,
        };

        serde_json::to_writer_pretty(contents, &desc).unwrap();

        Ok(())
    }
}

impl SchemeMut for Xhci {
    fn open(&mut self, path: &[u8], flags: usize, uid: u32, _gid: u32) -> Result<usize> {
        if uid != 0 { return Err(Error::new(EACCES)) }
        if flags & O_CREAT != 0 { return Err(Error::new(EINVAL) ) }

        let path_str = str::from_utf8(path).or(Err(Error::new(ENOENT)))?.trim_start_matches('/');

        if path_str.is_empty() {
            if flags & O_DIRECTORY != 0 || flags & O_STAT != 0 {
                let mut contents = Vec::new();

                for (index, _) in self.ports.iter().enumerate().filter(|(_, port)| port.flags().contains(port::PortFlags::PORT_CCS)) {
                    write!(contents, "port{}\n", index).unwrap();
                }

                let fd = self.next_handle;
                self.handles.insert(fd, Handle::TopLevel(0, contents));
                self.next_handle += 1;
                Ok(fd)
            } else {
                return Err(Error::new(EISDIR));
            }
        } else if path_str.starts_with("port") {
            let slash_idx = path_str.chars().position(|c| c == '/');

            let num_str = &path_str[4..slash_idx.unwrap_or(path_str.len())];
            let num = num_str.parse::<usize>().or(Err(Error::new(ENOENT)))?;

            let subdir_str = if slash_idx.is_some() && slash_idx.unwrap() + 1 < path_str.len() {
                Some(&path_str[slash_idx.unwrap() + 1..])
            } else {
                None
            };

            let handle = match subdir_str {
                Some("descriptors") => {
                    if flags & O_DIRECTORY != 0 {
                        return Err(Error::new(ENOTDIR));
                    }

                    let mut contents = Vec::new();
                    self.write_port_desc(num, &mut contents)?;

                    Handle::PortDesc(num, 0, contents)
                }
                Some(other) if other.contains('/') => {
                    let slash_idx = other.chars().position(|c| c == '/').ok_or(Error::new(ENOENT))?;
                    if slash_idx + 2 >= other.len() {
                        return Err(Error::new(ENOENT));
                    }

                    let slice = &other[slash_idx + 2..];
                    let endpoint_num = slice.parse::<usize>().or(Err(Error::new(ENOENT)))?;
                    Handle::Endpoint(num, endpoint_num, 0)
                }
                Some("endpoints") => {
                    if flags & O_DIRECTORY == 0 && flags & O_STAT == 0 {
                        return Err(Error::new(EISDIR));
                    };
                    let mut contents = Vec::new();
                    let ps = &self.port_states[&num];

                    for (ep_num, _) in ps.input_context.device.endpoints.iter().enumerate().filter(|(_, ep)| ep.a.read() & 0b111 == 1) {
                        write!(contents, "i{}", ep_num).unwrap();
                    }
                    for (ep_num, _) in self.dev_ctx.contexts[ps.slot as usize].endpoints.iter().enumerate().filter(|(_, ep)| ep.a.read() & 0b111 == 1) {
                        write!(contents, "o{}", ep_num).unwrap();
                    }

                    Handle::Endpoints(num, 0, contents)
                }
                Some(_) => return Err(Error::new(ENOENT)),
                None => if flags & O_DIRECTORY != 0 || flags & O_STAT != 0 {
                    let mut contents = Vec::new();

                    write!(contents, "descriptors\n").unwrap();

                    Handle::Port(num, 0, contents)
                } else {
                    return Err(Error::new(EISDIR));
                }
            };

            let fd = self.next_handle;
            self.next_handle += 1;
            self.handles.insert(fd, handle);

            Ok(fd)
        } else {
            return Err(Error::new(ENOSYS));
        }
    }

    fn fstat(&mut self, id: usize, stat: &mut Stat) -> Result<usize> {
        match self.handles.get(&id).ok_or(Error::new(EBADF))? {
            Handle::TopLevel(_, ref buf) | Handle::Port(_, _, ref buf) | Handle::Endpoints(_, _, ref buf) => {
                // TODO: Known size perhaps?
                stat.st_mode = MODE_DIR;
                stat.st_size = buf.len() as u64;
                Ok(0)
            }
            Handle::PortDesc(_, _, ref buf) => {
                stat.st_mode = MODE_FILE;
                stat.st_size = buf.len() as u64;
                Ok(0)
            }
            Handle::Endpoint(_, _, _) => {
                stat.st_mode = MODE_FILE;
                Ok(0)
            }
        }
    }

    fn seek(&mut self, fd: usize, pos: usize, whence: usize) -> Result<usize> {
        match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
            Handle::TopLevel(ref mut offset, ref buf) | Handle::Port(_, ref mut offset, ref buf) | Handle::PortDesc(_, ref mut offset, ref buf) | Handle::Endpoints(_, ref mut offset, ref buf) => {
                *offset = match whence {
                    SEEK_SET => cmp::max(0, cmp::min(pos, buf.len())),
                    SEEK_CUR => cmp::max(0, cmp::min(*offset + pos, buf.len())),
                    SEEK_END => cmp::max(0, cmp::min(buf.len() + pos, buf.len())),
                    _ => return Err(Error::new(EINVAL)),
                };
                Ok(*offset)
            }
            Handle::Endpoint(_, _, _) => unimplemented!(),
        }
    }

    fn read(&mut self, fd: usize, buf: &mut [u8]) -> Result<usize> {
        match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
            Handle::TopLevel(ref mut offset, ref src_buf) | Handle::Port(_, ref mut offset, ref src_buf) | Handle::PortDesc(_, ref mut offset, ref src_buf) | Handle::Endpoints(_, ref mut offset, ref src_buf) => {
                let max_bytes_to_read = cmp::min(src_buf.len(), buf.len());
                let bytes_to_read = cmp::max(max_bytes_to_read, *offset) - *offset;

                buf[..bytes_to_read].copy_from_slice(&src_buf[..bytes_to_read]);
                *offset += bytes_to_read;

                Ok(bytes_to_read)
            }
            Handle::Endpoint(_, _, _) => unimplemented!(),
        }
    }
    fn close(&mut self, fd: usize) -> Result<usize> {
        if self.handles.remove(&fd).is_none() {
            return Err(Error::new(EBADF));
        }
        Ok(0)
    }
}
