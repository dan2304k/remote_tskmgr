//! NVIDIA backend via NVML. `nvml-wrapper` loads `libnvidia-ml.so.1` / `nvml.dll` at runtime,
//! so a machine without the NVIDIA driver just fails `Nvml::init()` and Auto moves on.
//!
//! Hybrid laptops (Optimus): with the dGPU powered down at idle, `nvmlInit` and the device
//! count succeed but utilisation reads fail (GPU_IS_LOST, NOT_SUPPORTED, UNKNOWN) until the GPU
//! wakes. A device that enumerates is therefore accepted, and a failed read reports
//! `v: 0.0` with `detail.state = "asleep"` and `detail.nvml_error`, retried every tick.
//!
//! The device is the one matching the `gpu_adapter` target by name, else the k-th NVIDIA
//! device where k is the target's position among the NVIDIA adapters detected, else index 0.

use super::adapters::{adapters_detail, Adapter};
use super::{Ctx, GpuSource};
use crate::util::{mib, num};
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
use nvml_wrapper::error::NvmlError;
use nvml_wrapper::Nvml;
use serde_json::{Map, Value};

pub struct NvmlSource {
    nvml: Nvml,
    index: u32,
    name: String,
    list: Vec<Adapter>,
    target: Option<Adapter>,
}

/// Lowercase without "laptop gpu" and spaces, for loose name matching across APIs.
fn norm(s: &str) -> String {
    s.to_ascii_lowercase()
        .replace("laptop gpu", "")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

/// Which NVML device serves `target`, given the NVML device names in index order.
pub(crate) fn device_for(target: Option<&Adapter>, list: &[Adapter], names: &[String]) -> u32 {
    let Some(t) = target.filter(|t| t.is_nvidia()) else {
        return 0;
    };
    let tn = norm(&t.name);
    if let Some(i) = names.iter().position(|n| {
        let n = norm(n);
        !n.is_empty() && (n.contains(&tn) || tn.contains(&n))
    }) {
        return i as u32;
    }
    let k = list
        .iter()
        .filter(|a| a.is_nvidia())
        .position(|a| a.index == t.index)
        .unwrap_or(0);
    if k < names.len() {
        k as u32
    } else {
        0
    }
}

/// A utilisation read: the value, or 0.0 plus a state when the GPU is asleep / unreadable.
pub(crate) fn util_or_asleep(r: Result<u32, NvmlError>) -> (f64, Option<(&'static str, String)>) {
    match r {
        Ok(u) => (f64::from(u.min(100)), None),
        Err(e) => {
            let state = match e {
                NvmlError::GpuLost | NvmlError::NotSupported | NvmlError::Unknown => "asleep",
                _ => "unreadable",
            };
            (0.0, Some((state, format!("{e:?}"))))
        }
    }
}

impl NvmlSource {
    pub(crate) fn new(ctx: &Ctx) -> Result<Self, String> {
        let nvml = Nvml::init().map_err(|e| e.to_string())?;
        let count = nvml.device_count().map_err(|e| e.to_string())?;
        if count == 0 {
            return Err("no NVIDIA devices".into());
        }
        // Names work while the GPU sleeps.
        let names: Vec<String> = (0..count)
            .map(|i| {
                nvml.device_by_index(i)
                    .and_then(|d| d.name())
                    .unwrap_or_else(|_| "NVIDIA GPU".into())
            })
            .collect();
        let index = device_for(ctx.target.as_ref(), &ctx.list, &names);
        Ok(Self {
            name: names[index as usize].clone(),
            nvml,
            index,
            list: ctx.list.clone(),
            target: ctx.target.clone().filter(|t| t.is_nvidia()),
        })
    }
}

impl GpuSource for NvmlSource {
    fn backend(&self) -> &'static str {
        "nvml"
    }

    fn read(&mut self) -> (Option<f64>, Map<String, Value>) {
        let mut d = Map::new();
        d.insert("name".into(), Value::from(self.name.clone()));
        d.insert("nvml_index".into(), Value::from(self.index));
        if let Some(t) = &self.target {
            d.insert("adapter_index".into(), Value::from(t.index));
        }
        if !self.list.is_empty() {
            let chosen = self.target.as_ref().map(|t| t.index);
            d.insert(
                "adapters".into(),
                adapters_detail(&self.list, chosen, |_| Map::new()),
            );
        }
        let dev = self.nvml.device_by_index(self.index);
        let util = match &dev {
            Ok(dev) => util_or_asleep(dev.utilization_rates().map(|u| u.gpu)),
            Err(e) => util_or_asleep(Err(clone_err(e))),
        };
        if let Some((state, err)) = util.1 {
            d.insert("state".into(), Value::from(state));
            d.insert("nvml_error".into(), Value::from(err));
        }
        if let Ok(dev) = dev {
            if let Ok(m) = dev.memory_info() {
                d.insert("vram_used_mb".into(), mib(m.used));
                d.insert("vram_total_mb".into(), mib(m.total));
            }
            if let Ok(t) = dev.temperature(TemperatureSensor::Gpu) {
                d.insert("temp_c".into(), num(f64::from(t)));
            }
            if let Ok(f) = dev.clock_info(Clock::Graphics) {
                d.insert("freq_mhz".into(), Value::from(f));
            }
        }
        d.insert("backend".into(), Value::from(self.backend()));
        (Some(util.0), d)
    }
}

/// `NvmlError` is not `Clone`; keep the variants that matter for the asleep mapping.
fn clone_err(e: &NvmlError) -> NvmlError {
    match e {
        NvmlError::GpuLost => NvmlError::GpuLost,
        NvmlError::NotSupported => NvmlError::NotSupported,
        _ => NvmlError::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::adapters::fake;

    #[test]
    fn asleep_maps_to_zero() {
        assert_eq!(util_or_asleep(Ok(37)), (37.0, None));
        assert_eq!(util_or_asleep(Ok(250)).0, 100.0);
        for e in [
            NvmlError::GpuLost,
            NvmlError::NotSupported,
            NvmlError::Unknown,
        ] {
            let (v, st) = util_or_asleep(Err(e));
            assert_eq!(v, 0.0);
            assert_eq!(st.unwrap().0, "asleep");
        }
        let (v, st) = util_or_asleep(Err(NvmlError::Uninitialized));
        assert_eq!((v, st.unwrap().0), (0.0, "unreadable"));
    }

    #[test]
    fn device_follows_the_target() {
        let list = [
            fake(0, "Intel(R) UHD Graphics", "a", 0x8086, 128),
            fake(1, "NVIDIA GeForce RTX 3060 Laptop GPU", "b", 0x10de, 6144),
            fake(2, "NVIDIA RTX A2000", "c", 0x10de, 8192),
        ];
        let names = [
            "NVIDIA RTX A2000".to_string(),
            "NVIDIA GeForce RTX 3060".to_string(),
        ];
        assert_eq!(device_for(Some(&list[1]), &list, &names), 1, "by name");
        assert_eq!(device_for(Some(&list[2]), &list, &names), 0, "by name");
        let anon = ["NVIDIA GPU".to_string(), "NVIDIA GPU".to_string()];
        assert_eq!(
            device_for(Some(&list[2]), &list, &anon),
            1,
            "k-th NVIDIA adapter"
        );
        assert_eq!(
            device_for(Some(&list[0]), &list, &names),
            0,
            "not NVIDIA: default"
        );
        assert_eq!(device_for(None, &[], &names), 0);
    }
}
