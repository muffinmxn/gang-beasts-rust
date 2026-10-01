//! Gang Beasts' OpaqueSurfaceFog renderer feature, ported from its compiled shader
//! (`re/shaders/Hidden_GangBeasts_OpaqueSurfaceFog/0001_ps.asm`, see shaders/surface_fog.wgsl).
//! Data-driven per stage from `OpaqueSurfaceFogSceneSettings` + the stage sun, so every map gets
//! its own fog. Runs on the HDR scene after the main pass (before bloom/tonemapping), reading the
//! depth prepass. Put [`SurfaceFogPass`] (+ `DepthPrepass`) on the camera.
use bevy::{
    core_pipeline::{
        core_3d::graph::{Core3d, Node3d},
        fullscreen_vertex_shader::fullscreen_shader_vertex_state,
        prepass::ViewPrepassTextures,
    },
    ecs::query::QueryItem,
    prelude::*,
    render::{
        extract_component::{
            ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
            UniformComponentPlugin,
        },
        render_graph::{
            NodeRunError, RenderGraphApp, RenderGraphContext, RenderLabel, ViewNode, ViewNodeRunner,
        },
        render_resource::{
            binding_types::{sampler, texture_2d, texture_depth_2d, texture_depth_2d_multisampled, uniform_buffer},
            *,
        },
        renderer::{RenderContext, RenderDevice},
        view::{ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms},
        RenderApp,
    },
};

const SHADER: Handle<Shader> = bevy::asset::weak_handle!("7d2f4b18-3c9e-4a51-b6e2-0f1a9c3d5e77");

#[derive(Component, Clone, Copy, ExtractComponent, ShaderType, Default)]
pub struct SurfaceFogPass {
    pub fog_color: Vec4,
    pub light_color: Vec4,
    pub light_dir: Vec4,
    pub p: Vec4,
    pub q: Vec4,
}

impl SurfaceFogPass {
    /// `scene_scale`: our HDR scene value for one Unity lighting unit (sun calibration x exposure).
    pub fn new(fog: &crate::sky::SurfaceFog, sun: Option<(Color, f32, Vec3)>, scene_scale: f32, far: f32) -> Self {
        let lin = |c: Color| { let l = c.to_linear(); Vec4::new(l.red, l.green, l.blue, 1.0) };
        let (light_color, light_dir) = match sun {
            Some((c, intensity, to_light)) => (lin(c) * intensity * scene_scale, to_light.extend(0.0)),
            None => (Vec4::ZERO, Vec4::Y),
        };
        Self {
            fog_color: lin(fog.fog) * scene_scale,
            light_color,
            light_dir,
            p: Vec4::new(fog.scatter, fog.sky_max_depth, fog.sky_elevation, fog.ground_max_depth),
            q: Vec4::new(fog.ground_elevation, 0.1, far, 1.0),
        }
    }
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct SurfaceFogLabel;

pub struct SurfaceFogPlugin;

impl Plugin for SurfaceFogPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::load_internal_asset!(app, SHADER, "shaders/surface_fog.wgsl", Shader::from_wgsl);
        app.add_plugins((
            ExtractComponentPlugin::<SurfaceFogPass>::default(),
            UniformComponentPlugin::<SurfaceFogPass>::default(),
        ));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .add_render_graph_node::<ViewNodeRunner<FogNode>>(Core3d, SurfaceFogLabel)
            .add_render_graph_edges(Core3d, (Node3d::EndMainPass, SurfaceFogLabel, Node3d::Bloom));
    }

    fn finish(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app.init_resource::<FogPipeline>();
    }
}

#[derive(Default)]
struct FogNode;

impl ViewNode for FogNode {
    type ViewQuery = (
        &'static ViewTarget,
        &'static ViewPrepassTextures,
        &'static ViewUniformOffset,
        &'static SurfaceFogPass,
        &'static DynamicUniformIndex<SurfaceFogPass>,
        &'static Msaa,
    );

    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        ctx: &mut RenderContext,
        (target, prepass, view_offset, _fog, fog_index, msaa): QueryItem<Self::ViewQuery>,
        world: &World,
    ) -> Result<(), NodeRunError> {
        let res = world.resource::<FogPipeline>();
        let cache = world.resource::<PipelineCache>();
        let multisampled = msaa.samples() > 1;
        let id = match (target.is_hdr(), multisampled) {
            (true, true) => res.hdr_ms,
            (true, false) => res.hdr,
            (false, true) => res.ldr_ms,
            (false, false) => res.ldr,
        };
        let Some(pipeline) = cache.get_render_pipeline(id) else { return Ok(()) };
        let Some(depth) = prepass.depth_view() else { return Ok(()) };
        let Some(fog_binding) = world.resource::<ComponentUniforms<SurfaceFogPass>>().uniforms().binding() else {
            return Ok(());
        };
        let Some(view_binding) = world.resource::<ViewUniforms>().uniforms.binding() else { return Ok(()) };
        let post = target.post_process_write();
        let layout = if multisampled { &res.layout_ms } else { &res.layout };
        let bind_group = ctx.render_device().create_bind_group(
            "surface_fog_bind_group",
            layout,
            &BindGroupEntries::sequential((post.source, &res.sampler, fog_binding.clone(), view_binding.clone(), depth)),
        );
        let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("surface_fog"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: post.destination,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_render_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[fog_index.index(), view_offset.offset]);
        pass.draw(0..3, 0..1);
        Ok(())
    }
}

#[derive(Resource)]
struct FogPipeline {
    layout: BindGroupLayout,
    layout_ms: BindGroupLayout,
    sampler: Sampler,
    hdr: CachedRenderPipelineId,
    hdr_ms: CachedRenderPipelineId,
    ldr: CachedRenderPipelineId,
    ldr_ms: CachedRenderPipelineId,
}

impl FromWorld for FogPipeline {
    fn from_world(world: &mut World) -> Self {
        let device = world.resource::<RenderDevice>();
        let make = |ms: bool| {
            if ms {
                device.create_bind_group_layout("surface_fog_layout_ms", &BindGroupLayoutEntries::sequential(
                    ShaderStages::FRAGMENT,
                    (texture_2d(TextureSampleType::Float { filterable: true }), sampler(SamplerBindingType::Filtering),
                     uniform_buffer::<SurfaceFogPass>(true), uniform_buffer::<ViewUniform>(true),
                     texture_depth_2d_multisampled())))
            } else {
                device.create_bind_group_layout("surface_fog_layout", &BindGroupLayoutEntries::sequential(
                    ShaderStages::FRAGMENT,
                    (texture_2d(TextureSampleType::Float { filterable: true }), sampler(SamplerBindingType::Filtering),
                     uniform_buffer::<SurfaceFogPass>(true), uniform_buffer::<ViewUniform>(true),
                     texture_depth_2d())))
            }
        };
        let layout = make(false);
        let layout_ms = make(true);
        let sampler = device.create_sampler(&SamplerDescriptor::default());
        let mut queue = |layout: &BindGroupLayout, format: TextureFormat, ms: bool| {
            world.resource_mut::<PipelineCache>().queue_render_pipeline(RenderPipelineDescriptor {
                label: Some("surface_fog".into()),
                layout: vec![layout.clone()],
                vertex: fullscreen_shader_vertex_state(),
                fragment: Some(FragmentState {
                    shader: SHADER,
                    shader_defs: if ms { vec!["MULTISAMPLED".into()] } else { vec![] },
                    entry_point: "fragment".into(),
                    targets: vec![Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })],
                }),
                primitive: PrimitiveState::default(),
                depth_stencil: None,
                multisample: MultisampleState::default(),
                push_constant_ranges: vec![],
                zero_initialize_workgroup_memory: false,
            })
        };
        let hdr = queue(&layout, ViewTarget::TEXTURE_FORMAT_HDR, false);
        let hdr_ms = queue(&layout_ms, ViewTarget::TEXTURE_FORMAT_HDR, true);
        let ldr = queue(&layout, TextureFormat::bevy_default(), false);
        let ldr_ms = queue(&layout_ms, TextureFormat::bevy_default(), true);
        Self { layout, layout_ms, sampler, hdr, hdr_ms, ldr, ldr_ms }
    }
}
