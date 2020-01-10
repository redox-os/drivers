use std::{cmp, mem, str};
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
    endpoints: SmallVec<[EndpDescJson; 2]>,
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

        let config_descs = (0..raw_dd.configurations).map(|index| -> Result<_> {
            // TODO: Actually, it seems like all descriptors contain a length field, and I
            // encountered a SuperSpeed descriptor when endpoints were expected. The right way
            // would probably be to have an enum of all possible descs, and sort them based on
            // location, even though they might not necessarily be ordered trivially.

            let (desc, data) = dev.get_config(index)?;

            let extra_length = desc.total_length as usize - mem::size_of_val(&desc);

            let mut i = 0;

            let mut interface_descs = SmallVec::with_capacity(desc.interfaces as usize);

            for _ in 0..desc.interfaces {
                let mut idesc = usb::InterfaceDescriptor::default();
                if i < extra_length && i < data.len() && idesc.copy_from_bytes(&data[i..extra_length]).is_ok() {
                    i += mem::size_of_val(&idesc);

                    let mut endpoints = SmallVec::with_capacity(idesc.endpoints as usize);

                    while endpoints.len() < idesc.endpoints as usize {
                        let mut edesc = usb::EndpointDescriptor::default();
                        if i < extra_length && i < data.len() && edesc.copy_from_bytes(&data[i..extra_length]).is_ok() {
                            match edesc.kind {
                                // TODO: Constants
                                5 => i += mem::size_of_val(&edesc),
                                48 => { i += 6; continue } // SuperSpeed Endpoint Companion Descriptor
                                _ => unimplemented!(),
                            }

                            endpoints.push(EndpDescJson {
                                address: edesc.address,
                                attributes: edesc.attributes,
                                interval: edesc.interval,
                                kind: edesc.kind,
                                max_packet_size: edesc.max_packet_size,
                            })
                        } else { break }
                    }

                    interface_descs.push(IfDescJson {
                        kind: idesc.kind,
                        number: idesc.number,
                        alternate_setting: idesc.alternate_setting,
                        class: idesc.class,
                        sub_class: idesc.sub_class,
                        protocol: idesc.protocol,
                        interface_str: if idesc.interface_str > 0 { Some(dev.get_string(idesc.interface_str)?) } else { None },
                        endpoints,
                    });
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

            if slash_idx.is_some() && slash_idx.unwrap() + 1 < path_str.len() && &path_str[slash_idx.unwrap() + 1..] == "descriptors" {
                if flags & O_DIRECTORY != 0 {
                    return Err(Error::new(ENOTDIR));
                }

                let mut contents = Vec::new();
                self.write_port_desc(num, &mut contents)?;

                let fd = self.next_handle;
                self.handles.insert(fd, Handle::PortDesc(num, 0, contents));
                self.next_handle += 1;
                return Ok(fd);
            }

            if flags & O_DIRECTORY != 0 || flags & O_STAT != 0 {
                let mut contents = Vec::new();

                write!(contents, "descriptors\n").unwrap();

                let fd = self.next_handle;
                self.handles.insert(fd, Handle::Port(num, 0, contents));
                self.next_handle += 1;
                Ok(fd)
            } else {
                return Err(Error::new(EISDIR));
            }
        } else {
            return Err(Error::new(ENOSYS));
        }
    }

    fn fstat(&mut self, id: usize, stat: &mut Stat) -> Result<usize> {
        match self.handles.get(&id).ok_or(Error::new(EBADF))? {
            Handle::TopLevel(_, ref buf) | Handle::Port(_, _, ref buf) => {
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
        }
    }

    fn seek(&mut self, fd: usize, pos: usize, whence: usize) -> Result<usize> {
        match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
            Handle::TopLevel(ref mut offset, ref buf) | Handle::Port(_, ref mut offset, ref buf) | Handle::PortDesc(_, ref mut offset, ref buf) => {
                *offset = match whence {
                    SEEK_SET => cmp::max(0, cmp::min(pos, buf.len())),
                    SEEK_CUR => cmp::max(0, cmp::min(*offset + pos, buf.len())),
                    SEEK_END => cmp::max(0, cmp::min(buf.len() + pos, buf.len())),
                    _ => return Err(Error::new(EINVAL)),
                };
                Ok(*offset)
            }
        }
    }

    fn read(&mut self, fd: usize, buf: &mut [u8]) -> Result<usize> {
        match self.handles.get_mut(&fd).ok_or(Error::new(EBADF))? {
            Handle::TopLevel(ref mut offset, ref src_buf) | Handle::Port(_, ref mut offset, ref src_buf) | Handle::PortDesc(_, ref mut offset, ref src_buf) => {
                let max_bytes_to_read = cmp::min(src_buf.len(), buf.len());
                let bytes_to_read = cmp::max(max_bytes_to_read, *offset) - *offset;

                buf[..bytes_to_read].copy_from_slice(&src_buf[..bytes_to_read]);
                *offset += bytes_to_read;

                Ok(bytes_to_read)
            }
        }
    }
    fn close(&mut self, fd: usize) -> Result<usize> {
        if self.handles.remove(&fd).is_none() {
            return Err(Error::new(EBADF));
        }
        Ok(0)
    }
}
