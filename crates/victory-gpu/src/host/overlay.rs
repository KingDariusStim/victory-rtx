//! The title's second present plane shown over the first.
//!
//! The console scans out several planes at once: the game's picture, and
//! over it a plane of 2D (the menus, the title screen, the HUD). The title
//! presents both on every frame (run 38: two planes from the first present
//! on). Only the first was shown here, so every 2D screen before the world
//! reached the window black while the title drew it in full (run 37's dump
//! of the second plane's target at the title screen: a dark blue screen
//! with "ACCOUNT PICKER" and "PLAYER" and their buttons).
//!
//! A compute pass blends the second plane over the first into a texture of
//! the window's size and format, which is copied to the swap chain in place
//! of the first plane. The second plane is read with its alpha taken as
//! premultiplied (a console overlay plane's), scaled to the first's size.
//! shortcut: the planes' placement and source rectangles, colour spaces and
//! filters are not read.

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::{Host, transition};

const SOURCE: &str = r#"
#define RS "DescriptorTable(SRV(t0, numDescriptors = 2), UAV(u0)), RootConstants(num32BitConstants = 2, b0), StaticSampler(s0, filter = FILTER_MIN_MAG_MIP_LINEAR, addressU = TEXTURE_ADDRESS_CLAMP, addressV = TEXTURE_ADDRESS_CLAMP)"
Texture2D<float4> game : register(t0);
Texture2D<float4> plane : register(t1);
RWTexture2D<float4> shown : register(u0);
SamplerState linear_clamp : register(s0);
cbuffer Size : register(b0) { uint width; uint height; };

[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= width || id.y >= height) return;
    float2 uv = (float2(id.xy) + 0.5) / float2(width, height);
    float4 g = game.Load(int3(id.xy, 0));
    float4 p = plane.SampleLevel(linear_clamp, uv, 0);
    shown[id.xy] = float4(p.rgb + g.rgb * (1.0 - p.a), 1.0);
}
"#;

/// Descriptor sets (two views and the output) kept in turn: the card may
/// still read a set a few presents back while the next is written.
const SETS: u32 = 16;

fn pipeline(host: &Host) -> Option<&'static (ID3D12RootSignature, ID3D12PipelineState)> {
    static MADE: std::sync::OnceLock<Option<(ID3D12RootSignature, ID3D12PipelineState)>> = std::sync::OnceLock::new();
    MADE.get_or_init(|| {
        let made = (|| -> windows::core::Result<_> {
            let code = super::compile(SOURCE, windows::core::s!("main"), windows::core::s!("cs_5_1"))?;
            // SAFETY: the compiled blob holds the root signature; plain creation.
            unsafe {
                let root: ID3D12RootSignature = host.device.CreateRootSignature(0, &code)?;
                let pso = host.device.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(&root),
                    CS: D3D12_SHADER_BYTECODE { pShaderBytecode: code.as_ptr().cast(), BytecodeLength: code.len() },
                    ..Default::default()
                })?;
                Ok((root, pso))
            }
        })();
        match made {
            Ok(made) => Some(made),
            Err(e) => {
                tracing::warn!(error = %e, "no pipeline to show the second present plane: the first is shown alone");
                None
            }
        }
    })
    .as_ref()
}

/// A typed format to view a texture of `format` with in a shader.
fn viewed(format: DXGI_FORMAT) -> DXGI_FORMAT {
    match format {
        DXGI_FORMAT_R8G8B8A8_TYPELESS => DXGI_FORMAT_R8G8B8A8_UNORM,
        DXGI_FORMAT_B8G8R8A8_TYPELESS => DXGI_FORMAT_B8G8R8A8_UNORM,
        DXGI_FORMAT_R10G10B10A2_TYPELESS => DXGI_FORMAT_R10G10B10A2_UNORM,
        DXGI_FORMAT_R16G16B16A16_TYPELESS => DXGI_FORMAT_R16G16B16A16_FLOAT,
        f => f,
    }
}

/// The blend's views and output.
pub(super) struct Composite {
    heap: ID3D12DescriptorHeap,
    step: u32,
    next: u32,
    /// The output: texture, width, height, format.
    out: Option<(ID3D12Resource, u32, u32, DXGI_FORMAT)>,
}

impl Composite {
    pub(super) fn new(host: &Host) -> Option<Composite> {
        pipeline(host)?;
        // SAFETY: plain creation of a small shader-visible heap.
        unsafe {
            let heap: ID3D12DescriptorHeap = host
                .device
                .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    NumDescriptors: 3 * SETS,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                })
                .ok()?;
            let step = host.device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
            crate::host::name(&heap, || "the present planes' blend".to_owned());
            Some(Composite { heap, step, next: 0, out: None })
        }
    }

    /// The output of `width` x `height` in `format`, made if it is not (in
    /// the copy source state between presents).
    fn output(&mut self, host: &Host, width: u32, height: u32, format: DXGI_FORMAT) -> Option<ID3D12Resource> {
        if let Some((out, w, h, f)) = &self.out
            && (*w, *h, *f) == (width, height, format)
        {
            return Some(out.clone());
        }
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: u64::from(width),
            Height: height,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            ..Default::default()
        };
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
        let mut out: Option<ID3D12Resource> = None;
        // SAFETY: plain creation.
        unsafe {
            host.device
                .CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_SOURCE, None, &mut out)
                .ok()?;
        }
        let out = out?;
        crate::host::name(&out, || "the present planes blended".to_owned());
        // (One the card may still read stays alive: the size changes once
        // or twice in a run.)
        std::mem::forget(self.out.take());
        self.out = Some((out.clone(), width, height, format));
        Some(out)
    }

    /// Records into `list` the blend of `overlay` (in host state `over`)
    /// over `texture` (in `state`) into an output of the window's size and
    /// format: `texture` is left in the copy source state, `overlay` in
    /// its own, and the output, in the copy source state, is returned.
    /// `None`, recording nothing, where it cannot be done.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn record(
        &mut self,
        host: &Host,
        list: &ID3D12GraphicsCommandList,
        texture: &ID3D12Resource,
        state: D3D12_RESOURCE_STATES,
        overlay: &ID3D12Resource,
        over: D3D12_RESOURCE_STATES,
        (width, height): (u32, u32),
        format: DXGI_FORMAT,
    ) -> Option<ID3D12Resource> {
        // Formats a shader stores to typed everywhere.
        if !matches!(format, DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R10G10B10A2_UNORM | DXGI_FORMAT_R16G16B16A16_FLOAT) {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| tracing::warn!(format = format.0, "the second present plane is not shown over a window of this format"));
            return None;
        }
        let (root, pso) = pipeline(host)?;
        let out = self.output(host, width, height, format)?;
        let shader = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE;
        let set = self.next % SETS;
        self.next = self.next.wrapping_add(1);
        // SAFETY: our heap's slots of this set, live resources, an open list.
        unsafe {
            let cpu = self.heap.GetCPUDescriptorHandleForHeapStart();
            let at = |i: u32| D3D12_CPU_DESCRIPTOR_HANDLE { ptr: cpu.ptr + ((set * 3 + i) * self.step) as usize };
            for (i, t) in [texture, overlay].into_iter().enumerate() {
                let format = viewed(t.GetDesc().Format);
                let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                    Format: format,
                    ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                    Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                    Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                        Texture2D: D3D12_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1, ..Default::default() },
                    },
                };
                host.device.CreateShaderResourceView(t, Some(&desc), at(i as u32));
            }
            let uav = D3D12_UNORDERED_ACCESS_VIEW_DESC {
                Format: format,
                ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
                Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_UAV::default() },
            };
            host.device.CreateUnorderedAccessView(&out, None, Some(&uav), at(2));
            let mut before = vec![transition(&out, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS)];
            if state != shader {
                before.push(transition(texture, state, shader));
            }
            if over != shader {
                before.push(transition(overlay, over, shader));
            }
            list.ResourceBarrier(&before);
            list.SetDescriptorHeaps(&[Some(self.heap.clone())]);
            list.SetComputeRootSignature(root);
            list.SetPipelineState(pso);
            let gpu = self.heap.GetGPUDescriptorHandleForHeapStart();
            list.SetComputeRootDescriptorTable(0, D3D12_GPU_DESCRIPTOR_HANDLE { ptr: gpu.ptr + u64::from(set * 3 * self.step) });
            let size = [width, height];
            list.SetComputeRoot32BitConstants(1, 2, size.as_ptr().cast(), 0);
            list.Dispatch(width.div_ceil(8), height.div_ceil(8), 1);
            let mut after = vec![
                transition(&out, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE),
                transition(texture, shader, D3D12_RESOURCE_STATE_COPY_SOURCE),
            ];
            if over != shader {
                after.push(transition(overlay, shader, over));
            }
            list.ResourceBarrier(&after);
        }
        Some(out)
    }
}
