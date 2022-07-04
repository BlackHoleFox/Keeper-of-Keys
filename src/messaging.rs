use std::{
    marker::PhantomData,
    mem::{self, ManuallyDrop},
    os::raw::c_void,
    ptr,
    sync::Arc,
};

use bytemuck::Pod;
use core_foundation::{
    base::TCFType,
    data::{CFData, CFDataRef},
    declare_TCFType, impl_TCFType,
    runloop::{self, CFRunLoop, CFRunLoopSource},
    string::CFString,
};
use core_foundation_sys::messageport::{
    CFMessagePortContext, CFMessagePortCreateLocal, CFMessagePortCreateRemote,
    CFMessagePortCreateRunLoopSource, CFMessagePortGetTypeID, CFMessagePortRef,
    CFMessagePortSendRequest,
};

declare_TCFType!(CFMessagePort, CFMessagePortRef);
impl_TCFType!(CFMessagePort, CFMessagePortRef, CFMessagePortGetTypeID);

extern "C" fn arc_retain<F: Send + Sync + 'static>(data: *const c_void) -> *const c_void {
    // matching retain semantics requires that the refcount doesn't decrease
    // when adding to it.
    let data: ManuallyDrop<Arc<F>> = unsafe { ManuallyDrop::new(Arc::from_raw(data.cast())) };

    let clone = Arc::into_raw(Arc::clone(&data));

    clone.cast()
}

extern "C" fn arc_release<F: Send + Sync + 'static>(data: *const c_void) {
    let _data: Arc<F> = unsafe { Arc::from_raw(data.cast()) };
}

pub struct ReplyWith<A: Fn() + Send + Sync + 'static> {
    data: Option<Vec<u8>>,
    after_reply: Option<A>,
}

impl<A: Fn() + Send + Sync + 'static> ReplyWith<A> {
    pub fn new(data: Option<impl Pod>, after_reply: Option<A>) -> Self {
        let data = data.map(|d| bytemuck::bytes_of(&d).to_vec());
        Self { data, after_reply }
    }
}

pub struct Server<
    ToClient: Pod,
    FromClient: Pod,
    A: Fn() + Send + Sync + 'static,
    F: Fn(FromClient) -> ReplyWith<A> + Send + Sync + 'static,
> {
    msg_port: CFMessagePort,
    _msg_send: PhantomData<fn() -> ToClient>,
    _msg_recv: PhantomData<fn() -> FromClient>,
    _responder: PhantomData<F>,
}

impl<ToClient, FromClient, A, F> Server<ToClient, FromClient, A, F>
where
    ToClient: Pod,
    FromClient: Pod,
    A: Fn() + Send + Sync + 'static,
    F: Fn(FromClient) -> ReplyWith<A> + Send + Sync + 'static,
{
    pub fn create(name: &'static str, reply_with: F) -> Self {
        let ctx = CFMessagePortContext {
            version: 0, // per docs, must be zero
            info: Arc::into_raw(Arc::new(reply_with)) as *const Arc<F> as *mut c_void,
            retain: Some(arc_retain::<F>),
            release: Some(arc_release::<F>),
            copyDescription: None,
        };

        let name = CFString::from_static_string(name);

        let port = unsafe {
            CFMessagePortCreateLocal(
                ptr::null_mut(),
                name.as_concrete_TypeRef(),
                Some(Self::port_callback),
                &ctx,
                ptr::null_mut(),
            )
        };

        if !port.is_null() {
            let msg_port = unsafe { CFMessagePort::wrap_under_create_rule(port) };
            Self {
                msg_port,
                _msg_send: PhantomData,
                _msg_recv: PhantomData,
                _responder: PhantomData,
            }
        } else {
            panic!("failed to init server port");
        }
    }

    pub fn recv_messages(&mut self) {
        let rl_source = unsafe {
            CFRunLoopSource::wrap_under_create_rule(CFMessagePortCreateRunLoopSource(
                ptr::null_mut(),
                self.msg_port.as_concrete_TypeRef(),
                0, // per docs, must be zero
            ))
        };

        let current_loop = CFRunLoop::get_current();
        current_loop.add_source(&rl_source, unsafe { runloop::kCFRunLoopDefaultMode });
        CFRunLoop::run_current()
    }

    extern "C" fn port_callback(
        _local: CFMessagePortRef,
        msgid: i32,
        data: CFDataRef,
        info: *mut c_void,
    ) -> CFDataRef {
        log::debug!("received message port request with ID {msgid}");

        let msg_bytes = unsafe { CFData::wrap_under_get_rule(data) };
        let msg = match bytemuck::try_from_bytes(&msg_bytes) {
            Ok(msg) => msg,
            Err(_) => {
                log::warn!("received bogus event size");
                return ptr::null();
            }
        };

        let info: mem::ManuallyDrop<Arc<F>> =
            unsafe { mem::ManuallyDrop::new(Arc::from_raw(info.cast())) };

        let ReplyWith {
            data, after_reply, ..
        } = info(*msg);

        let reply = match data {
            Some(response) => {
                let response_data = CFData::from_buffer(&response);

                let data_ref = response_data.as_concrete_TypeRef();
                // the system releases the data object for us, how kind.
                mem::forget(response_data);

                data_ref
            }
            None => ptr::null(),
        };

        if let Some(after_reply) = after_reply {
            after_reply();
        }

        reply
    }
}

struct SendMesageResult;

impl SendMesageResult {
    #![allow(non_snake_case, non_upper_case_globals)]

    const kCFMessagePortSuccess: i32 = 0;
    const kCFMessagePortSendTimeout: i32 = -1;
    const kCFMessagePortReceiveTimeout: i32 = -2;
    const kCFMessagePortIsInvalid: i32 = -3;
    const kCFMessagePortTransportError: i32 = -4;
    const kCFMessagePortBecameInvalidError: i32 = -5;
}

pub struct Sender {
    msg_port: CFMessagePort,
}

impl Sender {
    pub fn connect(service: &'static str) -> Option<Self> {
        let name = CFString::from_static_string(service);

        let port =
            unsafe { CFMessagePortCreateRemote(ptr::null_mut(), name.as_concrete_TypeRef()) };

        if !port.is_null() {
            let msg_port = unsafe { CFMessagePort::wrap_under_create_rule(port) };
            Some(Self { msg_port })
        } else {
            None
        }
    }

    pub fn send<Send: Pod, Recv: Pod>(&mut self, message: Send) -> Recv {
        let send_data = CFData::from_buffer(bytemuck::bytes_of(&message));

        let mut ret: CFDataRef = ptr::null();

        let res = unsafe {
            CFMessagePortSendRequest(
                self.msg_port.as_concrete_TypeRef(),
                0, // this could be used for enum tagging
                send_data.as_concrete_TypeRef(),
                0.5,
                10.0,
                runloop::kCFRunLoopDefaultMode, // no reason for a different loop
                &mut ret,
            )
        };

        match res {
            SendMesageResult::kCFMessagePortSuccess => {
                assert!(!ret.is_null(), "mismatched request structure");
                let reply = unsafe { CFData::wrap_under_create_rule(ret) };
                *bytemuck::try_from_bytes(&reply).expect("mismatched reply structure")
            }
            SendMesageResult::kCFMessagePortSendTimeout => panic!("send timeout"),
            SendMesageResult::kCFMessagePortReceiveTimeout => panic!("receive timeout"),
            SendMesageResult::kCFMessagePortIsInvalid => panic!("invalid port"),
            SendMesageResult::kCFMessagePortTransportError => panic!("highway fell apart"),
            SendMesageResult::kCFMessagePortBecameInvalidError => panic!("port became bad"),
            _ => unreachable!(),
        }
    }
}
