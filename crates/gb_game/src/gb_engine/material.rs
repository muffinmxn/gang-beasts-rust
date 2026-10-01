//! `GbMaterial`: the Bevy material that runs the translated URP/GangBeasts shader.

use bevy::{
    asset::{load_internal_asset, weak_handle},
    pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin},
    prelude::*,
    render::{
        mesh::MeshVertexBufferLayoutRef,
        render_resource::{AsBindGroup, RenderPipelineDescriptor, ShaderRef, ShaderType, SpecializedMeshPipelineError},
    },
};

pub const GB_LIT_SHADER: Handle<Shader> = weak_handle!("3f0a1f5e-2b7c-4d1e-9a51-6c3c9b0d7e11");
pub const URP_POST_SHADER: Handle<Shader> = weak_handle!("8a4e2c11-5d6f-4b2a-8c3e-1f9d7b6a5e22");
pub const GB_TYPES_SHADER: Handle<Shader> = weak_handle!("5c7d9e21-3a4b-4c5d-8e6f-7a8b9c0d1e33");
pub const GB_PREPASS_SHADER: Handle<Shader> = weak_handle!("6d8e0f32-4b5c-4d6e-9f70-8b9c0d1e2f44");

pub const F_LIGHTMAP: u32 = 1;
pub const F_SHADOWMASK: u32 = 2;
pub const F_NORMALMAP: u32 = 4;
pub const F_ALPHACLIP: u32 = 8;
pub const F_UNLIT: u32 = 16;
pub const F_SKY: u32 = 32;
pub const F_RECEIVE_SHADOWS: u32 = 64;
pub const F_TRIPLANAR: u32 = 128;
pub const F_VERTEX_COLOR: u32 = 256;
pub const F_REFLECTION: u32 = 512;
pub const F_BASE_TEX: u32 = 1024;
pub const F_EMISSIVE_TEX: u32 = 2048;

#[derive(Clone, Copy, Debug, ShaderType)]
pub struct GbParams {
    pub base_color: Vec4,
    pub emissive: Vec4,
    pub lightmap_st: Vec4,
    pub surface: Vec4,
    pub base_st: Vec4,
    pub extra: Vec4,
    pub sh: [Vec4; 7],
    pub dir_shadow_strength: Vec4,
    pub dir_shadowmask_channel: Vec4,
    pub fog_color: Vec4,
    pub fog_params: Vec4,
    pub fog_sky: Vec4,
    pub fog_extra: Vec4,
    pub post: Vec4,
    /// white balance multiplier applied before post (rgb), a unused
    pub wb: Vec4,
    pub flags: u32,
    pub debug: u32,
    pub _pad0: u32,
    pub _pad1: u32,
}

impl Default for GbParams {
    fn default() -> Self {
        Self {
            base_color: Vec4::ONE,
            emissive: Vec4::ZERO,
            lightmap_st: Vec4::new(1.0, 1.0, 0.0, 0.0),
            surface: Vec4::new(0.0, 0.0, 1.0, 0.5),
            base_st: Vec4::new(1.0, 1.0, 0.0, 0.0),
            extra: Vec4::new(1.0, 1.0, 1.0, 1.0),
            sh: [Vec4::ZERO; 7],
            dir_shadow_strength: Vec4::ONE,
            dir_shadowmask_channel: Vec4::splat(-1.0),
            fog_color: Vec4::ZERO,
            fog_params: Vec4::ZERO,
            fog_sky: Vec4::ZERO,
            fog_extra: Vec4::ZERO,
            post: Vec4::new(1.0, 1.0, 1.0, 2.0),
            wb: Vec4::ONE,
            flags: F_RECEIVE_SHADOWS,
            debug: 0,
            _pad0: 0,
            _pad1: 0,
        }
    }
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
#[bind_group_data(GbMaterialKey)]
pub struct GbMaterial {
    #[uniform(0)]
    pub params: GbParams,
    #[texture(1)]
    #[sampler(2)]
    pub base: Option<Handle<Image>>,
    #[texture(3)]
    #[sampler(4)]
    pub normal: Option<Handle<Image>>,
    #[texture(5)]
    #[sampler(6)]
    pub lightmap: Option<Handle<Image>>,
    #[texture(7)]
    #[sampler(8)]
    pub shadowmask: Option<Handle<Image>>,
    #[texture(9, dimension = "cube")]
    #[sampler(10)]
    pub reflection: Option<Handle<Image>>,
    #[texture(11)]
    #[sampler(12)]
    pub emissive: Option<Handle<Image>>,
    pub alpha_mode: AlphaMode,
    pub double_sided: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct GbMaterialKey {
    double_sided: bool,
    sky: bool,
}

impl From<&GbMaterial> for GbMaterialKey {
    fn from(m: &GbMaterial) -> Self {
        Self { double_sided: m.double_sided, sky: m.params.flags & F_SKY != 0 }
    }
}

impl Material for GbMaterial {
    fn fragment_shader() -> ShaderRef {
        GB_LIT_SHADER.into()
    }
    fn prepass_fragment_shader() -> ShaderRef {
        GB_PREPASS_SHADER.into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }
    fn specialize(
        _pipeline: &MaterialPipeline<Self>,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if key.bind_group_data.double_sided || key.bind_group_data.sky {
            descriptor.primitive.cull_mode = None;
        }
        if key.bind_group_data.sky {
            if let Some(ds) = descriptor.depth_stencil.as_mut() {
                ds.depth_write_enabled = false;
            }
        }
        Ok(())
    }
}

pub struct GbMaterialPlugin;

impl Plugin for GbMaterialPlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(app, GB_TYPES_SHADER, "../shaders/gb/gb_types.wgsl", Shader::from_wgsl);
        load_internal_asset!(app, GB_PREPASS_SHADER, "../shaders/gb/gb_prepass.wgsl", Shader::from_wgsl);
        load_internal_asset!(app, URP_POST_SHADER, "../shaders/gb/urp_post_gb.wgsl", Shader::from_wgsl);
        load_internal_asset!(app, GB_LIT_SHADER, "../shaders/gb/gb_lit.wgsl", Shader::from_wgsl);
        app.add_plugins(MaterialPlugin::<GbMaterial>::default());
    }
}
