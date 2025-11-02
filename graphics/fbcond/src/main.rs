use event::EventQueue;
use inputd::ConsumerHandleEvent;
use libredox::errno::{EAGAIN, EINTR};
use orbclient::Event;
use redox_scheme::{CallRequest, RequestKind, Response, SignalBehavior, Socket};
use std::env;
use syscall::EVENT_READ;

use crate::scheme::{FbconScheme, VtIndex};

mod display;
mod scheme;
mod text;

fn main() {
    let vt_ids = env::args()
        .skip(1)
        .map(|arg| arg.parse().expect("invalid vt number"))
        .collect::<Vec<_>>();

    common::setup_logging(
        "graphics",
        "fbcond",
        "fbcond",
        common::output_level(),
        common::file_level()
    );

    redox_daemon::Daemon::new(|daemon| inner(daemon, &vt_ids)).expect("failed to create daemon");
}
fn inner(daemon: redox_daemon::Daemon, vt_ids: &[usize]) -> ! {
    let mut event_queue = EventQueue::new().expect("fbcond: failed to create event queue");

    // FIXME listen for resize events from inputd and handle them

    let mut socket = Socket::nonblock("fbcon").expect("fbcond: failed to create fbcon scheme");
    event_queue
        .subscribe(
            socket.inner().raw(),
            VtIndex::SCHEMA_SENTINEL,
            event::EventFlags::READ,
        )
        .expect("fbcond: failed to subscribe to scheme events");

    let mut scheme = FbconScheme::new(vt_ids, &mut event_queue);

    // This is not possible for now as fbcond needs to open new displays at runtime for graphics
    // driver handoff. In the future inputd may directly pass a handle to the display instead.
    //libredox::call::setrens(0, 0).expect("fbcond: failed to enter null namespace");

    daemon.ready().expect("failed to notify parent");

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
    socket: &mut Socket,
    scheme: &mut FbconScheme,
    blocked: &mut Vec<CallRequest>,
    event: VtIndex,
) {
    match event {
        VtIndex::SCHEMA_SENTINEL => {
            loop {
                let request = match socket.next_request(SignalBehavior::Restart) {
                    Ok(Some(request)) => request,
                    Ok(None) => {
                        // Scheme likely got unmounted
                        std::process::exit(0);
                    }
                    Err(err) if err.errno == EAGAIN => break,
                    Err(err) => panic!("vesad: failed to read display scheme: {err}"),
                };

                match request.kind() {
                    RequestKind::Call(call_request) => {
                        if let Some(resp) = call_request.handle_scheme_block(scheme) {
                            socket
                                .write_response(resp, SignalBehavior::Restart)
                                .expect("fbcond: failed to write display scheme");
                        } else {
                            blocked.push(call_request);
                        }
                    }
                    RequestKind::OnClose { id } => {
                        scheme.on_close(id);
                    }
                    RequestKind::Cancellation(cancellation_request) => {
                        if let Some(i) = blocked
                            .iter()
                            .position(|req| req.request().request_id() == cancellation_request.id)
                        {
                            let blocked_req = blocked.remove(i);
                            let resp = Response::new(&blocked_req, Err(syscall::Error::new(EINTR)));
                            socket
                                .write_response(resp, SignalBehavior::Restart)
                                .expect("vesad: failed to write display scheme");
                        }
                    }
                    _ => {}
                }
            }
        }
        vt_i => {
            let vt = scheme.vts.get_mut(&vt_i).unwrap();

            let mut events = [Event::new(); 16];
            loop {
                match vt
                    .display
                    .input_handle
                    .read_events(&mut events)
                    .expect("fbcond: Error while reading events")
                {
                    ConsumerHandleEvent::Events(&[]) => break,
                    ConsumerHandleEvent::Events(events) => {
                        for event in events {
                            vt.input(event)
                        }
                    }
                    ConsumerHandleEvent::Handoff => vt.handle_handoff(),
                }
            }
        }
    }

    // If there are blocked readers, try to handle them.
    {
        let mut i = 0;
        while i < blocked.len() {
            if let Some(resp) = blocked[i].handle_scheme_block(scheme) {
                socket
                    .write_response(resp, SignalBehavior::Restart)
                    .expect("vesad: failed to write display scheme");
                blocked.remove(i);
            } else {
                i += 1;
            }
        }
    }

    for (handle_id, handle) in scheme.handles.iter_mut() {
        if !handle.events.contains(EVENT_READ) {
            continue;
        }

        let can_read = scheme
            .vts
            .get(&handle.vt_i)
            .map_or(false, |console| console.can_read());

        if can_read {
            if !handle.notified_read {
                handle.notified_read = true;
                socket
                    .post_fevent(*handle_id, EVENT_READ.bits())
                    .expect("fbcond: failed to write display event");
            }
        } else {
            handle.notified_read = false;
        }
    }
}
