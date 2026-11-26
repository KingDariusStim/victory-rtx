//! Copies between a target in its 8-bit RGBA stand-in
//! ([`crate::resource::target_format`]) and texels in the title's 16-bit
//! colour format (B5G5R5A1 or B4G4R4A4), converted on the card instead of
//! skipped.
//!
//! A host that renders to neither format or writes neither as unordered
//! access (NVIDIA) holds such targets as R8G8B8A8_UNORM. The host cannot copy
//! between the two layouts, so a copy goes through two buffers of ours: the
//! source region is copied into the first (or read from the upload buffer
//! the title's footprint was filled into), a compute shader converts each
//! texel into the second, and that is copied into the destination. Buffers
//! are bound as root views: no descriptor slot is taken, no texture flag
//! changes. The pass sets a compute root signature and pipeline of its own,
//! so the list's next draw or dispatch sets the title's again
//! ([`crate::host::HostList::ours_recorded`]).

use std::sync::OnceLock;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::host::{HostList, transition};

const B5G5R5A1: u32 = DXGI_FORMAT_B5G5R5A1_UNORM.0 as u32;
const B4G4R4A4: u32 = DXGI_FORMAT_B4G4R4A4_UNORM.0 as u32;
const RGBA8: u32 = DXGI_FORMAT_R8G8B8A8_UNORM.0 as u32;

/// The conversion from `from` to `to` (formats), the shader's `mode`: 16-bit
/// to RGBA8 (0, 1) or RGBA8 to 16-bit (2, 3). None for any other pair.
pub(crate) fn mode(from: u32, to: u32) -> Option<u32> {
    match (from, to) {
        (B5G5R5A1, RGBA8) => Some(0),
        (B4G4R4A4, RGBA8) => Some(1),
        (RGBA8, B5G5R5A1) => Some(2),
        (RGBA8, B4G4R4A4) => Some(3),
        _ => None,
    }
}

/// Bytes per texel of the side a mode reads (`source`) or writes.
fn texel_bytes(mode: u32, source: bool) -> u32 {
    if (mode < 2) == source { 2 } else { 4 }
}

/// UNORM expansions and their rounding inverses, as the hardware converts:
/// 5 bits to 8 as round(v * 255 / 31) (bit replication, `v << 3 | v >> 2`,
/// is one lower for some values), 4 bits as
/// `v * 17`, back as round(x * 31 / 255) and round(x * 15 / 255); 1-bit alpha
/// set from 128 up. Each thread writes one dword of the destination: one
/// RGBA8 texel, or two 16-bit texels (the second left zero past the row's
/// end, in our buffer's row padding).
const SOURCE: &str = r#"
#define RS "RootConstants(num32BitConstants=12, b0), SRV(t0), UAV(u0)"
cbuffer C : register(b0) {
    uint mode; uint width; uint height; uint depth;
    uint src_x; uint src_y; uint src_z; uint src_pitch;
    uint src_slice; uint dst_pitch; uint dst_slice; uint unused;
};
ByteAddressBuffer src : register(t0);
RWByteAddressBuffer dst : register(u0);

uint source_address(uint x, uint y, uint z, uint bytes) {
    return (src_z + z) * src_slice + (src_y + y) * src_pitch + (src_x + x) * bytes;
}
uint load16(uint address) {
    uint w = src.Load(address & ~3u);
    return (address & 2u) != 0 ? (w >> 16) : (w & 0xffffu);
}
uint widen(uint v) {
    uint r, g, b, a;
    if (mode == 0) {
        b = v & 31u; g = (v >> 5) & 31u; r = (v >> 10) & 31u;
        r = (r * 255u + 15u) / 31u; g = (g * 255u + 15u) / 31u; b = (b * 255u + 15u) / 31u;
        a = (v >> 15) != 0 ? 255u : 0u;
    } else {
        b = (v & 15u) * 17u; g = ((v >> 4) & 15u) * 17u; r = ((v >> 8) & 15u) * 17u;
        a = ((v >> 12) & 15u) * 17u;
    }
    return r | (g << 8) | (b << 16) | (a << 24);
}
uint narrow(uint x) {
    uint r = x & 255u, g = (x >> 8) & 255u, b = (x >> 16) & 255u, a = x >> 24;
    if (mode == 2) {
        return ((b * 31u + 127u) / 255u) | (((g * 31u + 127u) / 255u) << 5)
            | (((r * 31u + 127u) / 255u) << 10) | ((a >= 128u ? 1u : 0u) << 15);
    }
    return ((b * 15u + 127u) / 255u) | (((g * 15u + 127u) / 255u) << 4)
        | (((r * 15u + 127u) / 255u) << 8) | (((a * 15u + 127u) / 255u) << 12);
}

[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    uint y = id.y, z = id.z;
    if (y >= height || z >= depth) return;
    uint row = z * dst_slice + y * dst_pitch;
    if (mode < 2) {
        uint x = id.x;
        if (x >= width) return;
        dst.Store(row + x * 4u, widen(load16(source_address(x, y, z, 2u))));
    } else {
        uint x = id.x * 2u;
        if (x >= width) return;
        uint lo = narrow(src.Load(source_address(x, y, z, 4u)));
        uint hi = x + 1u < width ? narrow(src.Load(source_address(x + 1u, y, z, 4u))) : 0u;
        dst.Store(row + x * 2u, lo | (hi << 16));
    }
}
"#;

fn pipeline() -> Option<&'static (ID3D12RootSignature, ID3D12PipelineState)> {
    static MADE: OnceLock<Option<(ID3D12RootSignature, ID3D12PipelineState)>> = OnceLock::new();
    MADE.get_or_init(|| {
        let host = crate::host::host()?;
        let made = (|| -> windows::core::Result<_> {
            let code = crate::host::compile(SOURCE, windows::core::s!("main"), windows::core::s!("cs_5_1"))?;
            // SAFETY: the compiled blob holds the root signature; plain creation.
            unsafe {
                let root: ID3D12RootSignature = host.device.CreateRootSignature(0, &code)?;
                let pso = host.device.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(&root),
                    CS: D3D12_SHADER_BYTECODE {
                        pShaderBytecode: code.as_ptr().cast(),
                        BytecodeLength: code.len(),
                    },
                    ..Default::default()
                })?;
                Ok((root, pso))
            }
        })();
        match made {
            Ok(made) => Some(made),
            Err(e) => {
                tracing::warn!(error = %e, "no 16-bit colour conversion pipeline: such copies stay skipped");
                None
            }
        }
    })
    .as_ref()
}

/// Where a converted copy reads: a host texture's subresource (in the copy
/// source state) and the box of it, or a host buffer holding texels in the
/// footprint's layout, from the box's corner.
pub(crate) enum Source<'a> {
    Texture { texture: &'a ID3D12Resource, sub: u32 },
    Buffer { buffer: &'a ID3D12Resource, offset: u64, format: u32, pitch: u32, height: u32 },
}

/// How many subresources a colour texture has: mips times array layers
/// (a volume's depth is not layers).
pub(crate) fn subresources(texture: &ID3D12Resource) -> u32 {
    // SAFETY: a live texture.
    let d = unsafe { texture.GetDesc() };
    let layers = if d.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D { 1 } else { u32::from(d.DepthOrArraySize) };
    u32::from(d.MipLevels) * layers
}

/// The width, height and depth of `texture`'s subresource `sub`.
fn extent(texture: &ID3D12Resource, sub: u32) -> Option<[u32; 3]> {
    let host = crate::host::host()?;
    let mut layout = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    // SAFETY: a live texture; one footprint into our own structure.
    unsafe {
        let desc = texture.GetDesc();
        host.device.GetCopyableFootprints(&desc, sub, 1, 0, Some(&mut layout), None, None, None);
    }
    let f = layout.Footprint;
    (f.Width > 0).then_some([f.Width, f.Height, f.Depth])
}

/// A placed footprint of a buffer of ours at offset 0, rows pitched for the
/// host.
fn footprint(format: u32, [width, height, depth]: [u32; 3], pitch: u32) -> D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
        Offset: 0,
        Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
            Format: DXGI_FORMAT(format as i32),
            Width: width,
            Height: height,
            Depth: depth,
            RowPitch: pitch,
        },
    }
}

fn buffer_location(buffer: &ID3D12Resource, f: D3D12_PLACED_SUBRESOURCE_FOOTPRINT) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: a borrowed pointer; the location never releases it.
        pResource: unsafe { std::mem::transmute_copy(buffer) },
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: f },
    }
}

fn texture_location(texture: &ID3D12Resource, sub: u32) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: a borrowed pointer; the location never releases it.
        pResource: unsafe { std::mem::transmute_copy(texture) },
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: sub },
    }
}

/// Records into `list` the copy of `source` (the region `region`, as
/// left, top, front, right, bottom, back; none: all of a texture
/// subresource) into `to` (a host texture in the copy destination state)
/// at subresource `to_sub` and corner `at`, converted between the 16-bit
/// format and RGBA8. False, recording nothing, if the two formats are not
/// such a pair or the parts cannot be made.
pub(crate) fn convert(
    list: &HostList,
    to: &ID3D12Resource,
    to_sub: u32,
    at: [u32; 3],
    source: Source<'_>,
    region: Option<[u32; 6]>,
) -> bool {
    let Some(host) = crate::host::host() else { return false };
    // SAFETY: a live texture.
    let to_format = unsafe { to.GetDesc() }.Format.0 as u32;
    let from_format = match &source {
        // SAFETY: a live texture.
        Source::Texture { texture, .. } => unsafe { texture.GetDesc() }.Format.0 as u32,
        Source::Buffer { format, .. } => *format,
    };
    let Some(mode) = mode(from_format, to_format) else { return false };
    let Some((root, pso)) = pipeline() else { return false };
    let [left, top, front, right, bottom, back] = match (region, &source) {
        (Some(r), _) => r,
        (None, Source::Texture { texture, sub }) => {
            let Some([w, h, d]) = extent(texture, *sub) else { return false };
            [0, 0, 0, w, h, d]
        }
        (None, Source::Buffer { .. }) => return false,
    };
    if right <= left || bottom <= top || back <= front {
        return true; // nothing to copy
    }
    let size = [right - left, bottom - top, back - front];
    let out_pitch = (size[0] * texel_bytes(mode, false)).next_multiple_of(256);
    let out_bytes = u64::from(out_pitch) * u64::from(size[1]) * u64::from(size[2]);
    let Some(out) = host.uav_buffer(out_bytes) else { return false };
    list.keep(&out);
    // The texels to read: our copy of the texture's region from its corner,
    // or the title's footprint (already in an upload buffer) from the box's.
    let (input, corner, in_pitch, in_slice) = match source {
        Source::Texture { texture, sub } => {
            let pitch = (size[0] * texel_bytes(mode, true)).next_multiple_of(256);
            let bytes = u64::from(pitch) * u64::from(size[1]) * u64::from(size[2]);
            let Some(input) = host.uav_buffer(bytes) else { return false };
            list.keep(&input);
            let to_input = buffer_location(&input, footprint(from_format, size, pitch));
            let from = texture_location(texture, sub);
            let b = D3D12_BOX { left, top, front, right, bottom, back };
            // SAFETY: an open list; our buffer and a live texture in the copy
            // source state; the box inside its subresource (the title's).
            unsafe {
                list.list.ResourceBarrier(&[transition(&input, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST)]);
                list.list.CopyTextureRegion(&to_input, 0, 0, 0, &from, Some(&b));
                list.list.ResourceBarrier(&[transition(
                    &input,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                )]);
                (input.GetGPUVirtualAddress(), [0, 0, 0], pitch, pitch * size[1])
            }
        }
        Source::Buffer { buffer, offset, pitch, height, .. } => {
            // SAFETY: a live upload buffer (always readable by shaders).
            let address = unsafe { buffer.GetGPUVirtualAddress() } + offset;
            (address, [left, top, front], pitch, pitch * height)
        }
    };
    let constants: [u32; 12] = [
        mode,
        size[0],
        size[1],
        size[2],
        corner[0],
        corner[1],
        corner[2],
        in_pitch,
        in_slice,
        out_pitch,
        out_pitch * size[1],
        0,
    ];
    let threads_x = if mode < 2 { size[0] } else { size[0].div_ceil(2) };
    let to_output = buffer_location(&out, footprint(to_format, size, out_pitch));
    let destination = texture_location(to, to_sub);
    // SAFETY: an open list; our root signature and pipeline (kept for the
    // process), our buffers kept by the list, a live destination texture in
    // the copy destination state.
    unsafe {
        list.list.ResourceBarrier(&[transition(&out, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_UNORDERED_ACCESS)]);
        list.list.SetComputeRootSignature(root);
        list.list.SetPipelineState(pso);
        list.list.SetComputeRoot32BitConstants(0, 12, constants.as_ptr().cast(), 0);
        list.list.SetComputeRootShaderResourceView(1, input);
        list.list.SetComputeRootUnorderedAccessView(2, out.GetGPUVirtualAddress());
        list.list.Dispatch(threads_x.div_ceil(8), size[1].div_ceil(8), size[2]);
        list.list.ResourceBarrier(&[transition(
            &out,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        )]);
        list.list.CopyTextureRegion(&destination, at[0], at[1], at[2], &to_output, None);
    }
    list.ours_recorded();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Host;

    fn texture(host: &Host, format: u32, width: u32, height: u32) -> ID3D12Resource {
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: u64::from(width),
            Height: height,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT(format as i32),
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            ..Default::default()
        };
        let mut made: Option<ID3D12Resource> = None;
        // SAFETY: plain creation.
        unsafe {
            host.device.CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_NONE,
                &desc,
                D3D12_RESOURCE_STATE_COPY_DEST,
                None,
                &mut made,
            )
        }
        .unwrap();
        made.unwrap()
    }

    fn barrier(list: &HostList, texture: &ID3D12Resource, before: D3D12_RESOURCE_STATES, after: D3D12_RESOURCE_STATES) {
        // SAFETY: an open list of ours; a texture of ours in `before`.
        unsafe { list.list.ResourceBarrier(&[transition(texture, before, after)]) };
    }

    /// Records into `list` a copy of `texture` (in the copy source state)
    /// into a read-back buffer: the buffer and its row pitch.
    fn read_back(host: &Host, list: &HostList, texture: &ID3D12Resource, bytes: u32) -> (ID3D12Resource, u32) {
        // SAFETY: a live texture.
        let d = unsafe { texture.GetDesc() };
        let pitch = (d.Width as u32 * bytes).next_multiple_of(256);
        let buffer = host.read_back_buffer(u64::from(pitch) * u64::from(d.Height)).unwrap();
        let to = buffer_location(&buffer, footprint(d.Format.0 as u32, [d.Width as u32, d.Height, 1], pitch));
        let from = texture_location(texture, 0);
        // SAFETY: an open list, our buffer and a texture in the copy source state.
        unsafe { list.list.CopyTextureRegion(&to, 0, 0, 0, &from, None) };
        (buffer, pitch)
    }

    fn mapped<T: Copy>(buffer: &ID3D12Resource, len: usize) -> Vec<T> {
        let mut at = std::ptr::null_mut();
        // SAFETY: a read-back buffer of at least `len` elements, read after
        // the list that wrote it ran.
        unsafe {
            buffer.Map(0, None, Some(&mut at)).unwrap();
            let v = std::slice::from_raw_parts(at.cast::<T>(), len).to_vec();
            buffer.Unmap(0, Some(&D3D12_RANGE::default()));
            v
        }
    }

    /// The expansion as the D3D UNORM rules give it: round(c * 255 / max).
    fn widened(format: u32, v: u16) -> u32 {
        let v = u32::from(v);
        let unorm = |c: u32, bits: u32| {
            let max = (1 << bits) - 1;
            (c * 255 + max / 2) / max
        };
        let (r, g, b, a) = if format == B5G5R5A1 {
            let a = if v >> 15 != 0 { 255 } else { 0 };
            (unorm((v >> 10) & 31, 5), unorm((v >> 5) & 31, 5), unorm(v & 31, 5), a)
        } else {
            (unorm((v >> 8) & 15, 4), unorm((v >> 4) & 15, 4), unorm(v & 15, 4), unorm(v >> 12, 4))
        };
        r | g << 8 | b << 16 | a << 24
    }

    /// What the card itself reads from `texture` (a 16-bit colour format, in
    /// a non-pixel shader resource state): each texel's channels times 255,
    /// rounded, as RGBA8 words, row after row without padding.
    fn sampled(host: &Host, texture: &ID3D12Resource, width: u32, height: u32) -> Vec<u32> {
        const SAMPLE: &str = "
            #define RS \"DescriptorTable(SRV(t0)), UAV(u0), RootConstants(num32BitConstants=1, b0)\"
            Texture2D<float4> t : register(t0);
            RWByteAddressBuffer o : register(u0);
            cbuffer C : register(b0) { uint width; };
            [RootSignature(RS)] [numthreads(8, 8, 1)]
            void main(uint3 id : SV_DispatchThreadID) {
                uint w, h; t.GetDimensions(w, h);
                if (id.x >= w || id.y >= h) return;
                uint4 c = (uint4)round(saturate(t.Load(int3(id.xy, 0))) * 255.0);
                o.Store((id.y * width + id.x) * 4, c.r | (c.g << 8) | (c.b << 16) | (c.a << 24));
            }";
        let code = crate::host::compile(SAMPLE, windows::core::s!("main"), windows::core::s!("cs_5_1")).unwrap();
        let bytes = u64::from(width * height * 4);
        let out = host.uav_buffer(bytes).unwrap();
        let back = host.read_back_buffer(bytes).unwrap();
        // SAFETY: plain creation; one view in a heap of ours; a list of ours
        // run to completion before its objects drop.
        unsafe {
            let root: ID3D12RootSignature = host.device.CreateRootSignature(0, &code).unwrap();
            let pso: ID3D12PipelineState = host
                .device
                .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(&root),
                    CS: D3D12_SHADER_BYTECODE { pShaderBytecode: code.as_ptr().cast(), BytecodeLength: code.len() },
                    ..Default::default()
                })
                .unwrap();
            let heap: ID3D12DescriptorHeap = host
                .device
                .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    NumDescriptors: 1,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    NodeMask: 0,
                })
                .unwrap();
            host.device.CreateShaderResourceView(texture, None, heap.GetCPUDescriptorHandleForHeapStart());
            let list = host.new_list().unwrap();
            let l = &list.list;
            l.SetDescriptorHeaps(&[Some(heap.clone())]);
            l.SetComputeRootSignature(&root);
            l.SetPipelineState(&pso);
            l.SetComputeRootDescriptorTable(0, heap.GetGPUDescriptorHandleForHeapStart());
            l.SetComputeRootUnorderedAccessView(1, out.GetGPUVirtualAddress());
            l.SetComputeRoot32BitConstant(2, width, 0);
            l.ResourceBarrier(&[transition(&out, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_UNORDERED_ACCESS)]);
            l.Dispatch(width.div_ceil(8), height.div_ceil(8), 1);
            l.ResourceBarrier(&[transition(&out, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
            l.CopyBufferRegion(&back, 0, &out, 0, bytes);
            list.close();
            host.execute(&[&list]);
        }
        mapped(&back, (width * height) as usize)
    }

    /// Every 16-bit value of both formats, in a texture of odd width (two
    /// texels per word on the way back, the last word's second half past the
    /// row): widened into the stand-in exactly as the card reads the 16-bit
    /// texture itself, and narrowed back to the same bits.
    #[test]
    fn every_16_bit_colour_survives_the_rgba8_stand_in_both_ways() {
        let Some(host) = crate::host::host() else {
            assert!(std::env::var_os("VICTORY_D3D12_WARP").is_none(), "WARP unavailable");
            return;
        };
        let (w, h) = (255u32, 258u32);
        let values: Vec<u16> = (0..w * h).map(|i| i as u16).collect();
        for format in [B5G5R5A1, B4G4R4A4] {
            let pitch = (w * 2).next_multiple_of(256);
            let (upload, data) = host.upload_buffer(u64::from(pitch * h)).unwrap();
            for y in 0..h {
                for x in 0..w {
                    let at = (y * pitch + x * 2) as usize;
                    // SAFETY: inside our mapped upload buffer.
                    unsafe { data.add(at).cast::<u16>().write_unaligned(values[(y * w + x) as usize]) };
                }
            }
            let wide = texture(host, RGBA8, w, h);
            let back = texture(host, format, w, h);
            let list = host.new_list().unwrap();
            let source = Source::Buffer { buffer: &upload, offset: 0, format, pitch, height: h };
            assert!(convert(&list, &wide, 0, [0; 3], source, Some([0, 0, 0, w, h, 1])));
            barrier(&list, &wide, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE);
            assert!(convert(&list, &back, 0, [0; 3], Source::Texture { texture: &wide, sub: 0 }, None));
            barrier(&list, &back, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE);
            let (wide_back, wide_pitch) = read_back(host, &list, &wide, 4);
            let (narrow_back, narrow_pitch) = read_back(host, &list, &back, 2);
            barrier(&list, &back, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
            list.close();
            host.execute(&[&list]);
            let wide_words: Vec<u32> = mapped(&wide_back, (wide_pitch / 4 * h) as usize);
            let narrow_words: Vec<u16> = mapped(&narrow_back, (narrow_pitch / 2 * h) as usize);
            // The card's own sampler may expand by bit replication instead
            // (NVIDIA, RTX 2080 SUPER: 5-bit 3 reads as 24/255, not 25): at
            // most one step of 255 per channel apart.
            let card = sampled(host, &back, w, h);
            let mut unlike_card = 0;
            for y in 0..h {
                for x in 0..w {
                    let v = values[(y * w + x) as usize];
                    let got = wide_words[(y * wide_pitch / 4 + x) as usize];
                    assert_eq!(got, widened(format, v), "format {format} value {v:#06x} widened");
                    let read = card[(y * w + x) as usize];
                    let near = (0..4).all(|c| ((got >> (8 * c)) & 255).abs_diff((read >> (8 * c)) & 255) <= 1);
                    assert!(near, "format {format} value {v:#06x}: ours {got:#010x}, the card's {read:#010x}");
                    unlike_card += usize::from(got != read);
                    assert_eq!(narrow_words[(y * narrow_pitch / 2 + x) as usize], v, "format {format} value {v:#06x} back");
                }
            }
            eprintln!("format {format}: {unlike_card} of {} texels one step off the card's own reading", w * h);
        }
    }

    /// A box of the title's footprint lands at the destination corner, the
    /// rest of the destination untouched.
    #[test]
    fn a_footprint_box_lands_at_the_destination_corner() {
        let Some(host) = crate::host::host() else {
            assert!(std::env::var_os("VICTORY_D3D12_WARP").is_none(), "WARP unavailable");
            return;
        };
        let (sw, sh, pitch) = (16u32, 12u32, 256u32);
        let (upload, data) = host.upload_buffer(u64::from(pitch * sh)).unwrap();
        let texel = |x: u32, y: u32| (0x8000 | (y << 10) | (x << 5) | 7) as u16;
        for y in 0..sh {
            for x in 0..sw {
                // SAFETY: inside our mapped upload buffer.
                unsafe { data.add((y * pitch + x * 2) as usize).cast::<u16>().write_unaligned(texel(x, y)) };
            }
        }
        let (dw, dh) = (20u32, 16u32);
        let wide = texture(host, RGBA8, dw, dh);
        let list = host.new_list().unwrap();
        let source = Source::Buffer { buffer: &upload, offset: 0, format: B5G5R5A1, pitch, height: sh };
        assert!(convert(&list, &wide, 0, [5, 4, 0], source, Some([3, 2, 0, 10, 7, 1])));
        barrier(&list, &wide, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE);
        let (back, back_pitch) = read_back(host, &list, &wide, 4);
        list.close();
        host.execute(&[&list]);
        let words: Vec<u32> = mapped(&back, (back_pitch / 4 * dh) as usize);
        for y in 0..dh {
            for x in 0..dw {
                let got = words[(y * back_pitch / 4 + x) as usize];
                let inside = (5..12).contains(&x) && (4..9).contains(&y);
                let want = if inside { widened(B5G5R5A1, texel(x - 5 + 3, y - 4 + 2)) } else { 0 };
                assert_eq!(got, want, "texel {x},{y}");
            }
        }
    }

    #[test]
    fn only_the_stand_in_pairs_convert() {
        assert_eq!(mode(B5G5R5A1, RGBA8), Some(0));
        assert_eq!(mode(RGBA8, B4G4R4A4), Some(3));
        assert_eq!(mode(RGBA8, RGBA8), None);
        assert_eq!(mode(B5G5R5A1, B4G4R4A4), None);
    }
}
