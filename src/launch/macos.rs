//! Receive Launch Services URLs without replacing winit's AppKit delegate.

use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained, sel};
use objc2_foundation::{
    MainThreadMarker, NSAppleEventDescriptor, NSAppleEventManager, NSObject, NSObjectProtocol,
};

use super::{Event, Sender};

const GET_URL: u32 = u32::from_be_bytes(*b"GURL");
const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

define_class!(
    // SAFETY: NSObject has no subclassing requirements. Apple Events are
    // delivered on the application main thread and both arguments are descriptors.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Sender]
    struct RustonUrlTarget;

    unsafe impl NSObjectProtocol for RustonUrlTarget {}

    impl RustonUrlTarget {
        #[unsafe(method(rustonHandleUrl:withReplyEvent:))]
        fn handle_url(&self, event: &NSAppleEventDescriptor, _reply: Option<&NSAppleEventDescriptor>) {
            if let Some(url) = event.paramDescriptorForKeyword(DIRECT_OBJECT).and_then(|param| param.stringValue()) {
                let request = (url.length() <= crate::mailto::MAX_URL_BYTES)
                    .then(|| crate::mailto::Request::parse(&url.to_string())).flatten();
                self.ivars().send(request.map(Event::Mailto).unwrap_or(Event::Rejected));
            }
        }
    }
);

pub(super) struct UrlEvents {
    // NSAppleEventManager's handler lifetime is explicitly tied to the instance.
    _target: Retained<RustonUrlTarget>,
    manager: Retained<NSAppleEventManager>,
}

impl UrlEvents {
    pub(super) fn install(sender: Sender) -> std::io::Result<Self> {
        let marker = MainThreadMarker::new()
            .ok_or_else(|| std::io::Error::other("URL reception requires the main thread"))?;
        let target = RustonUrlTarget::alloc(marker).set_ivars(sender);
        // SAFETY: inherited NSObject init has the expected signature.
        let target: Retained<RustonUrlTarget> = unsafe { msg_send![super(target), init] };
        let manager = NSAppleEventManager::sharedAppleEventManager();
        // SAFETY: the retained target implements the selector with two Apple
        // Event descriptor arguments and stays alive until it is unregistered.
        unsafe {
            manager.setEventHandler_andSelector_forEventClass_andEventID(
                &target,
                sel!(rustonHandleUrl:withReplyEvent:),
                GET_URL,
                GET_URL,
            );
        }
        Ok(Self {
            _target: target,
            manager,
        })
    }
}

impl Drop for UrlEvents {
    fn drop(&mut self) {
        self.manager
            .removeEventHandlerForEventClass_andEventID(GET_URL, GET_URL);
    }
}
