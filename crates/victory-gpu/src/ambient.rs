//! A least amount of indirect light while the title's rays do not run.
//!
//! The title's indirect diffuse light comes from probes it fills by tracing
//! rays, which do not run here yet: the program that resolves them
//! ([`PROGRAM`]) leaves next to nothing, and whatever the sun does not reach
//! directly is black. Right after that program, its output is replaced by a
//! constant; the title's own lighting then applies it with each surface's
//! colour. Replaced, not added to: without rays the resolve's inputs are
//! never written and its output is garbage (magenta on foliage, blocks of
//! colour indoors, as seen on AMD).
//!
//! `VICTORY_AMBIENT=<level>` sets the level (0.6 by default, 0 for none);
//! `off` keeps the title's own output. `VICTORY_AMBIENT_TINT=r,g,b` sets its
//! colour (warm white by default).
//!
//! Ported from the Belobug fork of VIctory
//! (github.com/Belobug/VIctory-Belobug-AMD, GPL-3.0-or-later, `ambient.rs`):
//! the program, the output's place and format, the level and the tint are
//! theirs; its depth shading, sky following and reflection hold are left out.
//! shortcut: the same light everywhere, from everywhere; gone once rays run.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::host::{HostList, host};
use crate::object::This;

/// The programs it follows, by the start of their name.
pub(crate) const PROGRAM: &[u8] = b"CS_ScreenProbes_TemporalResolve";
/// Where they write the indirect diffuse light (R9G9B9E5 as 32-bit
/// integers): root parameter and place in its table, in dwords.
pub(crate) const LIGHT: (u8, u32) = (1, 176);

const DEFAULT: f32 = 0.6;

/// The level; `None` if off.
pub(crate) fn level() -> Option<f32> {
    static LEVEL: OnceLock<f32> = OnceLock::new();
    let level = *LEVEL.get_or_init(|| {
        let set = std::env::var("VICTORY_AMBIENT").unwrap_or_default();
        if set.trim() == "off" {
            return -1.0;
        }
        set.trim().parse().ok().filter(|l: &f32| l.is_finite() && *l >= 0.0).unwrap_or(DEFAULT)
    });
    (level >= 0.0).then_some(level)
}

fn tint() -> [f32; 3] {
    static SET: OnceLock<[f32; 3]> = OnceLock::new();
    *SET.get_or_init(|| {
        let set: Vec<f32> = std::env::var("VICTORY_AMBIENT_TINT")
            .unwrap_or_default()
            .split(',')
            .filter_map(|v| v.trim().parse().ok())
            .collect();
        match set[..] {
            [r, g, b] if [r, g, b].iter().all(|c| c.is_finite() && *c >= 0.0) => [r, g, b],
            _ => [1.0, 0.98, 0.94],
        }
    })
}

const SOURCE: &str = r#"
#define RS "RootConstants(num32BitConstants=4, b0), DescriptorTable(UAV(u0))"
cbuffer Light : register(b0) { float3 light_colour; float unused; };
RWTexture2D<uint> light : register(u0);
uint pack(float3 c) {
    c = clamp(c, 0.0, 65408.0);
    float most = max(c.r, max(c.g, c.b));
    int e = clamp(int(floor(log2(max(most, 1e-9)))) + 16, 0, 31);
    float scale = exp2(float(24 - e));
    if (most * scale >= 511.5) { e = min(e + 1, 31); scale = exp2(float(24 - e)); }
    uint3 m = min(uint3(c * scale + 0.5), 511u);
    return m.r | (m.g << 9) | (m.b << 18) | (uint(e) << 27);
}
[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 at : SV_DispatchThreadID) {
    uint w, h;
    light.GetDimensions(w, h);
    if (at.x >= w || at.y >= h) return;
    light[at.xy] = pack(light_colour);
}
"#;

/// Views, used in turn: more than the passes of every list in flight.
const SLOTS: u32 = 1024;

struct Pass {
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    views: ID3D12DescriptorHeap,
    step: u32,
    next: AtomicU32,
}

fn make(device: &ID3D12Device) -> windows::core::Result<Pass> {
    let code = crate::host::compile(SOURCE, windows::core::s!("main"), windows::core::s!("cs_5_1"))?;
    // SAFETY: the root signature is inside the shader; plain creations.
    unsafe {
        let root: ID3D12RootSignature = device.CreateRootSignature(0, &code)?;
        let pso = device.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root),
            CS: D3D12_SHADER_BYTECODE { pShaderBytecode: code.as_ptr().cast(), BytecodeLength: code.len() },
            ..Default::default()
        })?;
        let kind = D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV;
        let views = device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
            Type: kind,
            NumDescriptors: SLOTS,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
            NodeMask: 0,
        })?;
        Ok(Pass { root, pso, views, step: device.GetDescriptorHandleIncrementSize(kind), next: AtomicU32::new(0) })
    }
}

fn pass() -> Option<&'static Pass> {
    static PASS: OnceLock<Option<Pass>> = OnceLock::new();
    PASS.get_or_init(|| {
        make(&host()?.device).inspect_err(|e| tracing::warn!(error = %e, "no ambient light pass")).ok()
    })
    .as_ref()
}

fn uav_barrier(list: &ID3D12GraphicsCommandList) {
    let barrier = D3D12_RESOURCE_BARRIER { Type: D3D12_RESOURCE_BARRIER_TYPE_UAV, ..Default::default() };
    // SAFETY: an open list; the barrier names no resource.
    unsafe { list.ResourceBarrier(&[barrier]) };
}

/// How often the light was set, for the log: [0] passes recorded, [1]
/// outputs of another format, [2] resolves without the output view.
pub(crate) static COUNTS: [std::sync::atomic::AtomicU64; 3] = [const { std::sync::atomic::AtomicU64::new(0) }; 3];

/// Sets the light in `light` (the title's resource the resolve just wrote,
/// in the unordered-access state it left it in), in `list`. Whether it
/// recorded anything: the list's heaps, root signature and pipeline are then
/// ours ([`HostList::ours_recorded`] is told).
pub(crate) fn raise(list: &HostList, light: This) -> bool {
    let (Some(level), Some(host), Some(pass)) = (level(), host(), pass()) else {
        return false;
    };
    let Some(texture) = crate::resource::host_resource(light) else {
        return false;
    };
    // SAFETY: a live resource.
    let d = unsafe { texture.GetDesc() };
    static ONCE: std::sync::Once = std::sync::Once::new();
    if d.Format != DXGI_FORMAT_R32_UINT || d.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D {
        COUNTS[1].fetch_add(1, Ordering::Relaxed);
        ONCE.call_once(|| tracing::warn!(format = d.Format.0, "no ambient light: the resolve's output has another format"));
        return false;
    }
    ONCE.call_once(|| tracing::info!(level, width = d.Width, height = d.Height, "ambient light set after the light resolve"));
    COUNTS[0].fetch_add(1, Ordering::Relaxed);
    list.keep(texture);
    let slot = pass.next.fetch_add(1, Ordering::Relaxed) % SLOTS;
    let l = &list.list;
    uav_barrier(l);
    // SAFETY: an open list; a live texture the list keeps, in the
    // unordered-access state the resolve left it in; a view in a slot no
    // list in flight reads (more slots than lists in flight).
    unsafe {
        let at = D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: pass.views.GetCPUDescriptorHandleForHeapStart().ptr + (slot * pass.step) as usize,
        };
        let view = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_UINT,
            ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
            ..Default::default()
        };
        host.device.CreateUnorderedAccessView(texture, None::<&ID3D12Resource>, Some(&view), at);
        l.SetDescriptorHeaps(&[Some(pass.views.clone())]);
        l.SetComputeRootSignature(&pass.root);
        l.SetPipelineState(&pass.pso);
        let [r, g, b] = tint();
        let constants = [r * level, g * level, b * level, 0.0];
        l.SetComputeRoot32BitConstants(0, 4, constants.as_ptr().cast(), 0);
        l.SetComputeRootDescriptorTable(
            1,
            D3D12_GPU_DESCRIPTOR_HANDLE {
                ptr: pass.views.GetGPUDescriptorHandleForHeapStart().ptr + u64::from(slot * pass.step),
            },
        );
        l.Dispatch((d.Width as u32).div_ceil(8), d.Height.div_ceil(8), 1);
    }
    uav_barrier(l);
    list.ours_recorded();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pass compiles and the host takes it; on the card, a 4x3 R32_UINT
    /// image is filled with the packed light (R9G9B9E5: 0.6 warm white).
    #[test]
    fn the_ambient_pass_fills_the_image_with_the_packed_light() {
        let Some(host) = host() else { return };
        let pass = pass().expect("the ambient pass");
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: 4,
            Height: 3,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_R32_UINT,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            ..Default::default()
        };
        let mut made: Option<ID3D12Resource> = None;
        // SAFETY: plain creation.
        unsafe {
            host.device
                .CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, None, &mut made)
                .unwrap();
        }
        let image = made.unwrap();
        let back = host.read_back_buffer(256 * 3).unwrap();
        let list = host.new_list().unwrap();
        // The pass without a title resource: the same commands `raise` records.
        // SAFETY: an open list of ours; our image and buffer.
        unsafe {
            let at = pass.views.GetCPUDescriptorHandleForHeapStart();
            let view = D3D12_UNORDERED_ACCESS_VIEW_DESC {
                Format: DXGI_FORMAT_R32_UINT,
                ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
                ..Default::default()
            };
            host.device.CreateUnorderedAccessView(&image, None::<&ID3D12Resource>, Some(&view), at);
            let l = &list.list;
            l.SetDescriptorHeaps(&[Some(pass.views.clone())]);
            l.SetComputeRootSignature(&pass.root);
            l.SetPipelineState(&pass.pso);
            let constants = [0.6f32, 0.588, 0.564, 0.0];
            l.SetComputeRoot32BitConstants(0, 4, constants.as_ptr().cast(), 0);
            l.SetComputeRootDescriptorTable(1, pass.views.GetGPUDescriptorHandleForHeapStart());
            l.Dispatch(1, 1, 1);
            l.ResourceBarrier(&[crate::host::transition(
                &image,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            )]);
            let to = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(&back),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                        Offset: 0,
                        Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                            Format: DXGI_FORMAT_R32_UINT,
                            Width: 4,
                            Height: 3,
                            Depth: 1,
                            RowPitch: 256,
                        },
                    },
                },
            };
            let from = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(&image),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
            };
            l.CopyTextureRegion(&to, 0, 0, 0, &from, None);
        }
        list.close();
        host.execute(&[&list]);
        let mut at = std::ptr::null_mut();
        // SAFETY: the read-back buffer, read after the list ran.
        let words: Vec<u32> = unsafe {
            back.Map(0, None, Some(&mut at)).unwrap();
            let rows = (0..3).flat_map(|y| std::slice::from_raw_parts(at.cast::<u32>().add(y * 64), 4).to_vec()).collect();
            back.Unmap(0, Some(&D3D12_RANGE::default()));
            rows
        };
        // R9G9B9E5: c = m * 2^(e - 24). 0.6 -> e 15, m 307 (0.5996); 0.588 -> 301; 0.564 -> 289.
        let unpack = |v: u32| {
            let s = (2f32).powi((v >> 27) as i32 - 24);
            [(v & 511) as f32 * s, ((v >> 9) & 511) as f32 * s, ((v >> 18) & 511) as f32 * s]
        };
        for w in words {
            let [r, g, b] = unpack(w);
            assert!((r - 0.6).abs() < 0.002 && (g - 0.588).abs() < 0.002 && (b - 0.564).abs() < 0.002, "{w:#010x}: {r} {g} {b}");
        }
    }
}
