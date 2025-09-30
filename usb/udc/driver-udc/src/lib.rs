use redox_scheme::{
    CallRequest, CallerCtx, OpenResult, RequestKind, Response, SchemeBlock, SignalBehavior, Socket,
};

pub trait UDCAdapter {
}

pub struct UDCScheme<T: NetworkAdapter> {
    udc: T,
}

impl<T: UDCAdapter> NetworkScheme<T> {
}

impl<T: UDCAdapter> SchemeBlock for NetworkScheme<T> {
}

impl<T: UDCAdapter> NetworkScheme<T> {
}
