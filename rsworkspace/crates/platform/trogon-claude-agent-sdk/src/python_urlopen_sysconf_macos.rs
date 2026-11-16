//! The proxy dictionary of the macOS system configuration, read through `SCDynamicStoreCopyProxies` as `_scproxy`
//! reads it: numbers converted by `CFNumberGetValue` to `SInt32`, strings as their UTF-16 code units.

use super::{ProtocolProxy, ProxyDictionary, ReadProxyDictionary};
use crate::constants::CF_NUMBER_SINT32_TYPE;
use std::ffi::c_void;

type CFTypeRef = *const c_void;
type CFIndex = isize;
type CFTypeID = usize;

#[repr(C)]
struct CFRange {
    location: CFIndex,
    length: CFIndex,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: CFTypeRef);
    fn CFGetTypeID(object: CFTypeRef) -> CFTypeID;
    fn CFDictionaryGetValue(dictionary: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn CFNumberGetTypeID() -> CFTypeID;
    fn CFNumberGetValue(number: CFTypeRef, kind: CFIndex, value: *mut c_void) -> u8;
    fn CFBooleanGetTypeID() -> CFTypeID;
    fn CFBooleanGetValue(boolean: CFTypeRef) -> u8;
    fn CFStringGetTypeID() -> CFTypeID;
    fn CFStringGetLength(string: CFTypeRef) -> CFIndex;
    fn CFStringGetCharacters(string: CFTypeRef, range: CFRange, buffer: *mut u16);
    fn CFArrayGetTypeID() -> CFTypeID;
    fn CFArrayGetCount(array: CFTypeRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(array: CFTypeRef, index: CFIndex) -> CFTypeRef;
}

#[link(name = "SystemConfiguration", kind = "framework")]
unsafe extern "C" {
    fn SCDynamicStoreCopyProxies(store: CFTypeRef) -> CFTypeRef;
    #[link_name = "kSCPropNetProxiesHTTPEnable"]
    static HTTP_ENABLE: CFTypeRef;
    #[link_name = "kSCPropNetProxiesHTTPProxy"]
    static HTTP_PROXY: CFTypeRef;
    #[link_name = "kSCPropNetProxiesHTTPPort"]
    static HTTP_PORT: CFTypeRef;
    #[link_name = "kSCPropNetProxiesHTTPSEnable"]
    static HTTPS_ENABLE: CFTypeRef;
    #[link_name = "kSCPropNetProxiesHTTPSProxy"]
    static HTTPS_PROXY: CFTypeRef;
    #[link_name = "kSCPropNetProxiesHTTPSPort"]
    static HTTPS_PORT: CFTypeRef;
    #[link_name = "kSCPropNetProxiesFTPEnable"]
    static FTP_ENABLE: CFTypeRef;
    #[link_name = "kSCPropNetProxiesFTPProxy"]
    static FTP_PROXY: CFTypeRef;
    #[link_name = "kSCPropNetProxiesFTPPort"]
    static FTP_PORT: CFTypeRef;
    #[link_name = "kSCPropNetProxiesGopherEnable"]
    static GOPHER_ENABLE: CFTypeRef;
    #[link_name = "kSCPropNetProxiesGopherProxy"]
    static GOPHER_PROXY: CFTypeRef;
    #[link_name = "kSCPropNetProxiesGopherPort"]
    static GOPHER_PORT: CFTypeRef;
    #[link_name = "kSCPropNetProxiesSOCKSEnable"]
    static SOCKS_ENABLE: CFTypeRef;
    #[link_name = "kSCPropNetProxiesSOCKSProxy"]
    static SOCKS_PROXY: CFTypeRef;
    #[link_name = "kSCPropNetProxiesSOCKSPort"]
    static SOCKS_PORT: CFTypeRef;
    #[link_name = "kSCPropNetProxiesExcludeSimpleHostnames"]
    static EXCLUDE_SIMPLE: CFTypeRef;
    #[link_name = "kSCPropNetProxiesExceptionsList"]
    static EXCEPTIONS: CFTypeRef;
}

/// The value of a `CFNumber` as `SInt32`, as `cfnum_to_int32` converts it, or of a `CFBoolean`.
///
/// # Safety
///
/// `value` is null or a live Core Foundation object.
unsafe fn number(value: CFTypeRef) -> Option<i32> {
    if value.is_null() {
        return None;
    }
    // SAFETY: `value` is a live object, whose type every Core Foundation object answers.
    let kind = unsafe { CFGetTypeID(value) };
    // SAFETY: the type identifiers are constant queries.
    let (number_type, boolean_type) = unsafe { (CFNumberGetTypeID(), CFBooleanGetTypeID()) };
    if kind == number_type {
        let mut converted: i32 = 0;
        // SAFETY: `value` is a `CFNumber` and `converted` has room for the `SInt32` asked for.
        unsafe { CFNumberGetValue(value, CF_NUMBER_SINT32_TYPE, (&raw mut converted).cast()) };
        Some(converted)
    } else if kind == boolean_type {
        // SAFETY: `value` is a `CFBoolean`.
        Some(i32::from(unsafe { CFBooleanGetValue(value) } != 0))
    } else {
        None
    }
}

/// The UTF-16 code units of a `CFString`.
///
/// # Safety
///
/// `value` is null or a live Core Foundation object.
unsafe fn text(value: CFTypeRef) -> Option<Vec<u16>> {
    // SAFETY: `value` is a live object and the type identifier is a constant query.
    if value.is_null() || unsafe { CFGetTypeID(value) != CFStringGetTypeID() } {
        return None;
    }
    // SAFETY: `value` is a `CFString`.
    let length = unsafe { CFStringGetLength(value) };
    let mut units = vec![0; usize::try_from(length).ok()?];
    // SAFETY: the range is the whole string and `units` holds one code unit for each of its characters.
    unsafe { CFStringGetCharacters(value, CFRange { location: 0, length }, units.as_mut_ptr()) };
    Some(units)
}

/// The strings of a `CFArray`, `None` for an element that is not one.
///
/// # Safety
///
/// `value` is null or a live Core Foundation object.
unsafe fn texts(value: CFTypeRef) -> Option<Vec<Option<Vec<u16>>>> {
    // SAFETY: `value` is a live object and the type identifier is a constant query.
    if value.is_null() || unsafe { CFGetTypeID(value) != CFArrayGetTypeID() } {
        return None;
    }
    // SAFETY: `value` is a `CFArray`.
    let count = unsafe { CFArrayGetCount(value) };
    // SAFETY: every index is below the count, and each element lives as long as the array.
    Some(
        (0..count)
            .map(|index| unsafe { text(CFArrayGetValueAtIndex(value, index)) })
            .collect(),
    )
}

/// The proxy dictionary of the current network configuration.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct DynamicStore;

impl ReadProxyDictionary for DynamicStore {
    fn read(&self) -> Option<ProxyDictionary> {
        // SAFETY: a null store asks for the proxies of the default session; the dictionary it returns is ours to
        // release.
        let dictionary = unsafe { SCDynamicStoreCopyProxies(std::ptr::null()) };
        if dictionary.is_null() {
            return None;
        }
        // SAFETY: the keys are the framework's constant strings, and each value lives until the dictionary is
        // released below.
        let value = |key: CFTypeRef| unsafe { CFDictionaryGetValue(dictionary, key) };
        // SAFETY: the keys are constant strings the framework exports.
        let keys = unsafe {
            [
                (HTTP_ENABLE, HTTP_PROXY, HTTP_PORT),
                (HTTPS_ENABLE, HTTPS_PROXY, HTTPS_PORT),
                (FTP_ENABLE, FTP_PROXY, FTP_PORT),
                (GOPHER_ENABLE, GOPHER_PROXY, GOPHER_PORT),
                (SOCKS_ENABLE, SOCKS_PROXY, SOCKS_PORT),
            ]
        };
        // SAFETY: every value is null or a live object of the dictionary.
        let read = unsafe {
            ProxyDictionary {
                protocols: keys.map(|(enable, host, port)| ProtocolProxy {
                    enable: number(value(enable)),
                    host: text(value(host)),
                    port: number(value(port)),
                }),
                exclude_simple: number(value(EXCLUDE_SIMPLE)),
                exceptions: texts(value(EXCEPTIONS)),
            }
        };
        // SAFETY: `dictionary` came from a copy function and is released once, after its last use.
        unsafe { CFRelease(dictionary) };
        Some(read)
    }
}
