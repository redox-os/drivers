use event::EventQueue;
use orbclient::Event;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::{env, io, mem, slice};
use syscall::{Packet, SchemeMut, EVENT_READ, O_NONBLOCK};

use crate::scheme::{FbconScheme, VtIndex};

mod display;
mod scheme;
mod text;

fn read_to_slice<R: Read, T: Copy>(mut r: R, buf: &mut [T]) -> io::Result<usize> {
    unsafe {
        r.read(slice::from_raw_parts_mut(
            buf.as_mut_ptr() as *mut u8,
            buf.len() * mem::size_of::<T>(),
        ))
        .map(|count| count / mem::size_of::<T>())
    }
}

fn main() {
    let vt_ids = env::args()
        .skip(1)
        .map(|arg| arg.parse().expect("invalid vt number"))
        .collect::<Vec<_>>();

    redox_daemon::Daemon::new(|daemon| inner(daemon, &vt_ids)).expect("failed to create daemon");
}
fn inner(daemon: redox_daemon::Daemon, vt_ids: &[usize]) -> ! {
    let mut event_queue = EventQueue::new().expect("fbcond: failed to create event queue");

    // FIXME listen for resize events from inputd and handle them

    let mut socket = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(O_NONBLOCK as i32)
        .open(":fbcon")
        .expect("fbcond: failed to create fbcon scheme");
    event_queue
        .subscribe(
            socket.as_raw_fd().as_raw_fd() as usize,
            VtIndex::SCHEMA_SENTINEL,
            event::EventFlags::READ,
        )
        .expect("fbcond: failed to subscribe to scheme events");

    let mut scheme = FbconScheme::new(vt_ids, &mut event_queue);

    libredox::call::setrens(0, 0).expect("fbcond: failed to enter null namespace");

    daemon.ready().expect("failed to notify parent");

    scheme.inputd_handle.activate(1).unwrap();

    let mut blocked = Vec::new();

    // Handle all events that could have happened before registering with the event queue.
    handle_event(
        &mut socket,
        &mut scheme,
        &mut blocked,
        VtIndex::SCHEMA_SENTINEL,
    );
    for vt_i in scheme.vts.keys().copied().collect::<Vec<_>>() {
        handle_event(&mut socket, &mut scheme, &mut blocked, vt_i);
    }

    for event in event_queue {
        let event = event.expect("fbcond: failed to read event from event queue");
        handle_event(&mut socket, &mut scheme, &mut blocked, event.user_data);
    }

    std::process::exit(0);
}

fn handle_event(
    socket: &mut File,
    scheme: &mut FbconScheme,
    blocked: &mut Vec<Packet>,
    event: VtIndex,
) {
    match event {
        VtIndex::SCHEMA_SENTINEL => {
            loop {
                let mut packet = Packet::default();
                match socket.read(&mut packet) {
                    Ok(0) => break,
                    Err(err) if err.kind() == ErrorKind::WouldBlock => {
                        break;
                    }
                    Ok(_) => {}
                    Err(err) => {
                        panic!("fbcond: failed to read display scheme: {err}");
                    }
                }

                // If it is a read packet, and there is no data, block it. Otherwise, handle packet
                if packet.a == syscall::number::SYS_READ
                    && packet.d > 0
                    && scheme.can_read(packet.b).is_none()
                {
                    blocked.push(packet);
                } else {
                    scheme.handle(&mut packet);
                    socket
                        .write(&packet)
                        .expect("fbcond: failed to write display scheme");
                }
            }
        }
        vt_i => {
            let vt = scheme.vts.get_mut(&vt_i).unwrap();

            let mut events = [Event::new(); 16];
            loop {
                match read_to_slice(&mut vt.display.input_handle, &mut events) {
                    Ok(0) => break,
                    Err(err) if err.kind() == ErrorKind::WouldBlock => {
                        break;
                    }

                    Ok(count) => {
                        let events = &mut events[..count];
                        for event in events.iter_mut() {
                            vt.input(event)
                        }
                    }
                    Err(err) => {
                        panic!("fbcond: Error while reading events: {err}");
                    }
                }
            }
        }
    }

    // If there are blocked readers, and data is available, handle them
    {
        let mut i = 0;
        while i < blocked.len() {
            if scheme.can_read(blocked[i].b).is_some() {
                let mut packet = blocked.remove(i);
                scheme.handle(&mut packet);
                socket
                    .write(&packet)
                    .expect("fbcond: failed to write display scheme");
            } else {
                i += 1;
            }
        }
    }

    for (handle_id, handle) in scheme.handles.iter_mut() {
        if !handle.events.contains(EVENT_READ) {
            continue;
        }

        // Can't use scheme.can_read() because we borrow handles as mutable.
        // (and because it'd treat O_NONBLOCK sockets differently)
        let count = scheme
            .vts
            .get(&handle.vt_i)
            .and_then(|console| console.can_read())
            .unwrap_or(0);

        if count > 0 {
            if !handle.notified_read {
                handle.notified_read = true;
                let event_packet = Packet {
                    id: 0,
                    pid: 0,
                    uid: 0,
                    gid: 0,
                    a: syscall::number::SYS_FEVENT,
                    b: *handle_id,
                    c: EVENT_READ.bits(),
                    d: count,
                };

                socket
                    .write(&event_packet)
                    .expect("fbcond: failed to write display event");
            }
        } else {
            handle.notified_read = false;
        }
    }
}
