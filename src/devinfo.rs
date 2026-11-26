//! Small helpers for asking Windows about a USB device.

use windows::core::{GUID, HSTRING, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_DevNode_PropertyW, CM_Locate_DevNodeW, CM_LOCATE_DEVNODE_NORMAL, CR_SUCCESS,
};
use windows::Win32::Devices::Properties::DEVPROPTYPE;
use windows::Win32::Foundation::DEVPROPKEY;

/// `\\?\usb#vid_534d&pid_2109&mi_00#6&13bd6ac2&0&0000#{...}\global`
/// becomes `usb\vid_534d&pid_2109&mi_00\6&13bd6ac2&0&0000`.
fn instance_id_from_link(link: &str) -> Option<String> {
    let s = link.strip_prefix(r"\\?\").unwrap_or(link);
    let end = s.find("#{")?;
    Some(s[..end].replace('#', "\\"))
}

/// The container id of a video device: every part of one physical gadget
/// (its camera, its microphone) shares the same one.
pub fn container_of_video(link: &str) -> Option<GUID> {
    let id = instance_id_from_link(link)?;
    unsafe {
        let mut inst = 0u32;
        let h = HSTRING::from(id);
        if CM_Locate_DevNodeW(&mut inst, PCWSTR(h.as_ptr()), CM_LOCATE_DEVNODE_NORMAL) != CR_SUCCESS {
            return None;
        }
        let key = DEVPROPKEY { fmtid: GUID::from_u128(0x8c7ed206_3f8a_4827_b3ab_ae9e1faefc6c), pid: 2 };
        let mut ty = DEVPROPTYPE::default();
        let mut g = GUID::zeroed();
        let mut size = std::mem::size_of::<GUID>() as u32;
        let r = CM_Get_DevNode_PropertyW(inst, &key, &mut ty, Some(&mut g as *mut GUID as *mut u8), &mut size, 0);
        (r == CR_SUCCESS).then_some(g)
    }
}
