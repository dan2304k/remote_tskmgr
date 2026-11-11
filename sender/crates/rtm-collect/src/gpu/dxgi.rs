//! Windows adapter list: DXGI first (name + LUID + dedicated VRAM in one call), the registry
//! display-class `DriverDesc` values as a fallback (names only, no LUID).

use super::adapters::{guess_integrated, Adapter};
use super::pdh_parse::luid_key;
use windows::core::{w, HSTRING};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

pub fn list() -> Vec<Adapter> {
    match from_dxgi() {
        Ok(v) if !v.is_empty() => v,
        Ok(_) => from_registry(),
        Err(e) => {
            tracing::debug!("gpu: DXGI adapter list failed ({e}); using the registry");
            from_registry()
        }
    }
}

fn from_dxgi() -> windows::core::Result<Vec<Adapter>> {
    let mut out = Vec::new();
    // SAFETY: plain COM calls on interfaces we own; DXGI needs no CoInitialize.
    unsafe {
        let f: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut i = 0;
        while let Ok(a) = f.EnumAdapters1(i) {
            i += 1;
            let Ok(d) = a.GetDesc1() else { continue };
            if d.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 {
                continue; // Microsoft Basic Render Driver
            }
            let len = d.Description.iter().position(|&c| c == 0).unwrap_or(128);
            let name = String::from_utf16_lossy(&d.Description[..len])
                .trim()
                .to_string();
            let vram = Some(d.DedicatedVideoMemory as u64);
            let vendor = u16::try_from(d.VendorId).ok();
            out.push(Adapter {
                index: out.len(),
                name,
                id: luid_key(d.AdapterLuid.HighPart as u32, d.AdapterLuid.LowPart),
                vendor,
                vram_total: vram,
                integrated: guess_integrated(vendor, vram),
            });
        }
    }
    Ok(out)
}

/// `HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-...}\00NN\DriverDesc`.
fn from_registry() -> Vec<Adapter> {
    let mut out: Vec<Adapter> = Vec::new();
    for n in 0..32 {
        let key = HSTRING::from(format!(
            "SYSTEM\\CurrentControlSet\\Control\\Class\\{{4d36e968-e325-11ce-bfc1-08002be10318}}\\{n:04}"
        ));
        let mut buf = [0u16; 256];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: buf/size describe a valid writable buffer.
        let st = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                &key,
                w!("DriverDesc"),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if st.is_err() {
            continue;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let name = String::from_utf16_lossy(&buf[..len]).trim().to_string();
        if name.is_empty() || name.contains("Basic Render") || name.contains("Basic Display") {
            continue;
        }
        out.push(Adapter {
            index: out.len(),
            name,
            id: format!("reg{n:04}"),
            vendor: None,
            vram_total: None,
            integrated: false,
        });
    }
    out
}
