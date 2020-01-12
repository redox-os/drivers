use std::{cmp, mem, path, str};
use std::io::prelude::*;

use serde::{Serialize, Deserialize};
use smallvec::SmallVec;

use syscall::io::{Dma, Io};
use syscall::scheme::SchemeMut;
use syscall::{
    EACCES, EBADF, EBADMSG, EINVAL, EISDIR, ENOENT, ENOSYS, ENOTDIR, ENXIO, ESPIPE,
    O_CREAT, O_DIRECTORY, O_STAT,
    MODE_DIR, MODE_FILE,
    SEEK_CUR, SEEK_END, SEEK_SET,
    Stat,
    Error, Result
};

use super::{Device, Xhci};
use super::{port, usb};
use super::context::{ENDPOINT_CONTEXT_STATUS_MASK, InputContext};

/// Subdirs of an endpoint
enum EndpointState {
    /// `/portX/endpoints/Y/init`, used for a one-write-call initialization of an endpoint.
    Init, 

    /// portX/endpoints/Y/transfer. Write calls transfer data to the device, and read calls
    /// transfer data from the device.
    Transfer, 

    /// portX/endpoints/Y/status
    Status(usize), // offset

    /// portX/endpoints/Y/
    Root(usize, Vec<u8>), // offset, content
}

pub enum Handle {
    TopLevel(usize, Vec<u8>), // offset, contents (ports)
    Port(usize, usize, Vec<u8>), // port, offset, contents
    PortDesc(usize, usize, Vec<u8>), // port, offset, contents
    Endpoints(usize, usize, Vec<u8>), // port, offset, contents
    Endpoint(usize, usize, EndpointState), // port, endpoint, offset, state
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

enum AnyEndpDescJson {
    Endp(EndpDescJson),
    SuperSpeedCmp(SuperSpeedCmpJson),
}
impl serde::Serialize for AnyEndpDescJson {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Endp(e) => e.serialize(serializer),
            Self::SuperSpeedCmp(c) => c.serialize(serializer),
        }
    }
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

#[derive(Deserialize)]
struct InitEndpointReqJson {
    index: u8,
}

impl Xhci {
    fn init_endpoint(&mut self, buf: &[u8]) -> Result<()> {
        let req: InitEndpointReqJson = serde_json::from_slice(buf).or(Err(Error::new(EBADMSG)))?;
        let input_context = Dma::<InputContext>::zeroed()?;

        // Endpoint zero is the control endpoint, which is always enabled.
        if !(1..=31).contains(&req.index) { return Err(Error::new(EINVAL)) }

        input_context.add_context.write(1 << req.index);
        // FIXME: A port string is required
        //input_context.device.slot.

        Ok(())
    }
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
                            let next = match iter.next() {
                                Some(AnyDescriptor::SuperSpeedCompanion(n)) => n,
                                _ => break,
                            };
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

        let components = path::Path::new(path_str).components().map(|component| -> Option<_> {
            match component {
                path::Component::Normal(n) => Some(n.to_str()?),
                _ => None,
            }
        }).collect::<Option<SmallVec<[&str; 4]>>>().ok_or(Error::new(ENOENT))?;

        let handle = match &components[..] {
            &[] => if flags & O_DIRECTORY != 0 || flags & O_STAT != 0 {
                let mut contents = Vec::new();

                for (index, _) in self.ports.iter().enumerate().filter(|(_, port)| port.flags().contains(port::PortFlags::PORT_CCS)) {
                    write!(contents, "port{}\n", index).unwrap();
                }

                Handle::TopLevel(0, contents)
            } else {
                return Err(Error::new(EISDIR));
            }
            &[port, "descriptors"] if port.starts_with("port") => {
                let port_num = port[4..].parse::<usize>().or(Err(Error::new(ENOENT)))?;

                if flags & O_DIRECTORY != 0 {
                    return Err(Error::new(ENOTDIR));
                }

                let mut contents = Vec::new();
                self.write_port_desc(port_num, &mut contents)?;

                Handle::PortDesc(port_num, 0, contents)
            }
            &[port, "endpoints"] if port.starts_with("port") => {
                let port_num = port[4..].parse::<usize>().or(Err(Error::new(ENOENT)))?;

                if flags & O_DIRECTORY == 0 && flags & O_STAT == 0 {
                    return Err(Error::new(EISDIR));
                };
                let mut contents = Vec::new();
                let ps = self.port_states.get(&port_num).ok_or(Error::new(ENOENT))?;

                for (ep_num, _) in self.dev_ctx.contexts[ps.slot as usize].endpoints.iter().enumerate().filter(|(_, ep)| ep.a.read() & 0b111 == 1) {
                    write!(contents, "{}\n", ep_num).unwrap();
                }

                Handle::Endpoints(port_num, 0, contents)
            }
            &[port, "endpoints", endpoint_num_str] if port.starts_with("port") => {
                let port_num = port[4..].parse::<usize>().or(Err(Error::new(ENOENT)))?;
                let endpoint_num = endpoint_num_str.parse::<usize>().or(Err(Error::new(ENOENT)))?;

                if flags & O_DIRECTORY == 0 && flags & O_STAT == 0 {
                    return Err(Error::new(EISDIR));
                }

                if flags & O_CREAT != 0 {
                    Handle::Endpoint(port_num, endpoint_num, EndpointState::Init)
                } else {
                    if self.dev_ctx.contexts[self.port_states.get(&port_num).ok_or(Error::new(ENOENT))?.slot as usize].endpoints.get(endpoint_num).ok_or(Error::new(ENOENT))?.a.read() & 0b111 != 1 {
                        return Err(Error::new(ENXIO)); // TODO: Find a proper error code for "endpoint not initialized".
                    }
                    let contents = b"transfer\nstatus"[..].to_owned();
                    Handle::Endpoint(port_num, endpoint_num, EndpointState::Root(0, contents))
                }
            }
            &[port, "endpoints", endpoint_num_str, sub] if port.starts_with("port") => {
                let port_num = port[4..].parse::<usize>().or(Err(Error::new(ENOENT)))?;
                let endpoint_num = endpoint_num_str.parse::<usize>().or(Err(Error::new(ENOENT)))?;

                if flags & O_DIRECTORY != 0 && flags & O_STAT == 0 { return Err(Error::new(EISDIR)) }

                let st = match sub {
                    "status" => EndpointState::Status(0),
                    "transfer" => EndpointState::Transfer,
                    _ => return Err(Error::new(ENOENT)),
                };
                Handle::Endpoint(port_num, endpoint_num, st)
            }
            &[port] if port.starts_with("port") => if flags & O_DIRECTORY != 0 || flags & O_STAT != 0 {
                let port_num = port[4..].parse::<usize>().or(Err(Error::new(ENOENT)))?;
                let mut contents = Vec::new();

                write!(contents, "descriptors\nendpoints\n").unwrap();

                Handle::Port(port_num, 0, contents)
            } else {
                return Err(Error::new(EISDIR));
            }
            _ => return Err(Error::new(ENOENT)),
        };

        let fd = self.next_handle;
        self.next_handle += 1;
        self.handles.insert(fd, handle);

        Ok(fd)
    }

    fn fstat(&mut self, id: usize, stat: &mut Stat) -> Result<usize> {
        match self.handles.get(&id).ok_or(Error::new(EBADF))? {
            Handle::TopLevel(_, ref buf) | Handle::Port(_, _, ref buf) | Handle::Endpoints(_, _, ref buf) => {
                // TODO: Known size perhaps?
                stat.st_mode = MODE_DIR;
                stat.st_size = buf.len() as u64;
            }
            Handle::PortDesc(_, _, ref buf) => {
                stat.st_mode = MODE_FILE;
                stat.st_size = buf.len() as u64;
            }
            Handle::Endpoint(_, _, st) => match st {
                EndpointState::Init | EndpointState::Status(_) | EndpointState::Transfer => stat.st_mode = MODE_FILE,
                EndpointState::Root(_, _) => stat.st_mode = MODE_DIR,
            }
        }
        Ok(0)
    }

    fn fpath(&mut self, fd: usize, buffer: &mut [u8]) -> Result<usize> {
        // XXX: write!() should return the length instead of ().
        let mut src = Vec::<u8>::new();
        match self.handles.get(&fd).ok_or(Error::new(EBADF))? {
            Handle::TopLevel(_, _) => write!(src, "/").unwrap(),
            Handle::Port(port_num, _, _) => write!(src, "/port{}/", port_num).unwrap(),
            Handle::PortDesc(port_num, _, _) => write!(src, "/port{}/descriptors", port_num).unwrap(),
            Handle::Endpoints(port_num, _, _) => write!(src, "/port{}/endpoints/", port_num).unwrap(),
            Handle::Endpoint(port_num, endp_num, st) => write!(src, "/port{}/endpoints/{}/{}", port_num, endp_num, match st {
                EndpointState::Init => "init",
                EndpointState::Root(_, _) => "",
                EndpointState::Status(_) => "status",
                EndpointState::Transfer => "transfer",
            }).unwrap(),
        }
        let bytes_to_read = cmp::min(src.len(), buffer.len());
        buffer[..bytes_to_read].copy_from_slice(&src[..bytes_to_read]);
        Ok(bytes_to_read)
    }

    fn seek(&mut self, fd: usize, pos: usize, whence: usize) -> Result<usize> {
        match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
            Handle::TopLevel(ref mut offset, ref buf) | Handle::Port(_, ref mut offset, ref buf) | Handle::PortDesc(_, ref mut offset, ref buf) | Handle::Endpoints(_, ref mut offset, ref buf) | Handle::Endpoint(_, _, EndpointState::Root(ref mut offset, ref buf)) => {
                *offset = match whence {
                    SEEK_SET => cmp::max(0, cmp::min(pos, buf.len())),
                    SEEK_CUR => cmp::max(0, cmp::min(*offset + pos, buf.len())),
                    SEEK_END => cmp::max(0, cmp::min(buf.len() + pos, buf.len())),
                    _ => return Err(Error::new(EINVAL)),
                };
                Ok(*offset)
            }
            Handle::Endpoint(_, _, _) => return Err(Error::new(ESPIPE)),
        }
    }

    fn read(&mut self, fd: usize, buf: &mut [u8]) -> Result<usize> {
        match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
            Handle::TopLevel(ref mut offset, ref src_buf) | Handle::Port(_, ref mut offset, ref src_buf) | Handle::PortDesc(_, ref mut offset, ref src_buf) | Handle::Endpoints(_, ref mut offset, ref src_buf) | Handle::Endpoint(_, _, EndpointState::Root(ref mut offset, ref src_buf)) => {
                let max_bytes_to_read = cmp::min(src_buf.len(), buf.len());
                let bytes_to_read = cmp::max(max_bytes_to_read, *offset) - *offset;

                buf[..bytes_to_read].copy_from_slice(&src_buf[..bytes_to_read]);
                *offset += bytes_to_read;

                Ok(bytes_to_read)
            }
            &mut Handle::Endpoint(port_num, endp_num, ref mut st) => match st {
                EndpointState::Init => return Err(Error::new(EINVAL)),
                EndpointState::Transfer => unimplemented!(),
                EndpointState::Status(ref mut offset) => {
                    let status = self.dev_ctx.contexts.get(port_num).ok_or(Error::new(EBADF))?.endpoints.get(endp_num).ok_or(Error::new(EBADF))?.a.read() & ENDPOINT_CONTEXT_STATUS_MASK;

                    let string = match status {
                        // TODO: Give this its own enum.
                        0 => "disabled",
                        1 => "enabled",
                        2 => "halted",
                        3 => "stopped",
                        4 => "error",
                        _ => "unknown",
                    }.as_bytes();

                    let max_bytes_to_read = cmp::min(string.len(), buf.len());
                    let bytes_to_read = cmp::max(*offset, max_bytes_to_read) - *offset;
                    buf[..bytes_to_read].copy_from_slice(&string[..bytes_to_read]);

                    *offset += bytes_to_read;

                    Ok(bytes_to_read)
                }
                EndpointState::Root(_, _) => unreachable!(),
            },
        }
    }
    fn write(&mut self, fd: usize, buf: &[u8]) -> Result<usize> {
        match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
            Handle::Endpoint(_, _, EndpointState::Init) => {
                self.init_endpoint(buf)?;
                Ok(buf.len())
            };
            Handle::Endpoint(_, _, EndpointState::Transfer) => unimplemented!(),
            _ => return Err(Error::new(EINVAL)),
        }
    }
    fn close(&mut self, fd: usize) -> Result<usize> {
        if self.handles.remove(&fd).is_none() {
            return Err(Error::new(EBADF));
        }
        Ok(0)
    }
}
