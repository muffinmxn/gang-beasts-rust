//! URP 12 post colour pipeline (postExposure, ACEScc contrast, saturation, reference ACES
//! tonemap) as a Bevy render-graph pass after the (disabled) Bevy tonemapper. Put [`UrpPost`] on a
//! camera with `Tonemapping::None` and identity `ColorGrading`. Shader: shaders/urp_post.wgsl.
#![allow(dead_code)] // ShaderType derive emits unused per-field helpers
use bevy::{
    core_pipeline::{
        core_3d::graph::{Core3d, Node3d},
        fullscreen_vertex_shader::fullscreen_shader_vertex_state,
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
            binding_types::{sampler, texture_2d, uniform_buffer},
            *,
        },
        renderer::{RenderContext, RenderDevice},
        view::ViewTarget,
        RenderApp,
    },
};

/// URP Volume ColorAdjustments + ACES tonemapping for one camera.
#[derive(Component, Clone, Copy, ExtractComponent, ShaderType)]
pub struct UrpPost {
    /// 2^postExposure (EV).
    pub exposure: f32,
    /// 1 + contrast / 100.
    pub contrast: f32,
    /// 1 + saturation / 100.
    pub saturation: f32,
    /// 1 = pass the image through (camera uses a Bevy tonemapper instead).
    pub bypass: f32,
    /// >1 darkens the low end (deepens shadows); 1 = off.
    pub shadow: f32,
    /// Upper luminance bound of the shadow-deepening ramp. Lower = only the
    /// darkest occluded areas respond; higher = more of the low end.
    pub shadow_knee: f32,
    /// Chromatic aberration intensity (0 = off; the graphics-options row writes this).
    pub ca: f32,
    /// Vignette intensity (0 = off).
    pub vignette: f32,
    /// Film grain intensity (0 = off).
    pub grain: f32,
    /// Grain animation seed (seconds).
    pub grain_time: f32,
    pub _pad0: f32,
    pub _pad1: f32,
}

impl UrpPost {
    pub fn new(post_exposure_ev: f32, contrast: f32, saturation: f32) -> Self {
        Self {
            exposure: 2f32.powf(post_exposure_ev),
            contrast: 1.0 + contrast / 100.0,
            saturation: 1.0 + saturation / 100.0,
            bypass: 0.0,
            // Harness knobs (no rebuild needed): GB_POST_SHADOW deepens only the
            // occluded low end; GB_POST_SHADOW_KNEE moves that ramp's top
            // (0.45 default ≈ Unity's visible-shade boundary; lower keeps mids).
            shadow: env_f32("GB_POST_SHADOW", 1.0),
            shadow_knee: env_f32("GB_POST_SHADOW_KNEE", 0.45),
            // Screen-space effects default OFF: every harness baseline (and the tuned title
            // match) was captured without them. The graphics settings screen turns them on;
            // GB_POST_CA / GB_POST_VIGNETTE / GB_POST_GRAIN preset them for captures.
            ca: env_f32("GB_POST_CA", 0.0),
            vignette: env_f32("GB_POST_VIGNETTE", 0.0),
            grain: env_f32("GB_POST_GRAIN", 0.0),
            grain_time: 0.0,
            _pad0: 0.0,
            _pad1: 0.0,
        }
    }

    pub fn bypass() -> Self {
        Self {
            exposure: 1.0,
            contrast: 1.0,
            saturation: 1.0,
            bypass: 1.0,
            shadow: 1.0,
            shadow_knee: 0.45,
            ca: 0.0,
            vignette: 0.0,
            grain: 0.0,
            grain_time: 0.0,
            _pad0: 0.0,
            _pad1: 0.0,
        }
    }
}

fn env_f32(key: &str, default: f32) -> f32 {
    // Through the knob store so the in-game panel (F2) can change it live.
    crate::devgui::knob(key, default)
}

pub struct UrpPostPlugin;

const SHADER: Handle<Shader> = bevy::asset::weak_handle!("5e4a2c31-7b1d-4f0e-9a6b-3c2d1e0f9a8b");

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct UrpPostLabel;

impl Plugin for UrpPostPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::load_internal_asset!(app, SHADER, "shaders/urp_post.wgsl", Shader::from_wgsl);
        app.add_plugins((
            ExtractComponentPlugin::<UrpPost>::default(),
            UniformComponentPlugin::<UrpPost>::default(),
        ));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .add_render_graph_node::<ViewNodeRunner<UrpPostNode>>(Core3d, UrpPostLabel)
            .add_render_graph_edges(
                Core3d,
                (Node3d::Tonemapping, UrpPostLabel, Node3d::EndMainPassPostProcessing),
            );
    }

    fn finish(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app.init_resource::<UrpPostPipeline>();
    }
}

#[derive(Default)]
struct UrpPostNode;

impl ViewNode for UrpPostNode {
    type ViewQuery = (
        &'static ViewTarget,
        &'static UrpPost,
        &'static DynamicUniformIndex<UrpPost>,
    );

    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        (view_target, _settings, index): QueryItem<Self::ViewQuery>,
        world: &World,
    ) -> Result<(), NodeRunError> {
        let pipeline_res = world.resource::<UrpPostPipeline>();
        let cache = world.resource::<PipelineCache>();
        let id = if view_target.is_hdr() {
            pipeline_res.hdr
        } else {
            pipeline_res.ldr
        };
        let Some(pipeline) = cache.get_render_pipeline(id) else {
            return Ok(());
        };
        let uniforms = world.resource::<ComponentUniforms<UrpPost>>();
        let Some(binding) = uniforms.uniforms().binding() else {
            return Ok(());
        };
        let post = view_target.post_process_write();
        let bind_group = render_context.render_device().create_bind_group(
            "urp_post_bind_group",
            &pipeline_res.layout,
            &BindGroupEntries::sequential((post.source, &pipeline_res.sampler, binding.clone())),
        );
        let mut pass = render_context.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("urp_post"),
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
        pass.set_bind_group(0, &bind_group, &[index.index()]);
        pass.draw(0..3, 0..1);
        Ok(())
    }
}

#[derive(Resource)]
struct UrpPostPipeline {
    layout: BindGroupLayout,
    sampler: Sampler,
    hdr: CachedRenderPipelineId,
    ldr: CachedRenderPipelineId,
}

impl FromWorld for UrpPostPipeline {
    fn from_world(world: &mut World) -> Self {
        let device = world.resource::<RenderDevice>();
        let layout = device.create_bind_group_layout(
            "urp_post_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                    uniform_buffer::<UrpPost>(true),
                ),
            ),
        );
        let sampler = device.create_sampler(&SamplerDescriptor::default());
        let shader = SHADER;
        let mut queue = |format: TextureFormat, label: &'static str| {
            world
                .resource_mut::<PipelineCache>()
                .queue_render_pipeline(RenderPipelineDescriptor {
                    label: Some(label.into()),
                    layout: vec![layout.clone()],
                    vertex: fullscreen_shader_vertex_state(),
                    fragment: Some(FragmentState {
                        shader: shader.clone(),
                        shader_defs: vec![],
                        entry_point: "fragment".into(),
                        targets: vec![Some(ColorTargetState {
                            format,
                            blend: None,
                            write_mask: ColorWrites::ALL,
                        })],
                    }),
                    primitive: PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: MultisampleState::default(),
                    push_constant_ranges: vec![],
                    zero_initialize_workgroup_memory: false,
                })
        };
        let hdr = queue(ViewTarget::TEXTURE_FORMAT_HDR, "urp_post_hdr");
        let ldr = queue(TextureFormat::bevy_default(), "urp_post_ldr");
        Self {
            layout,
            sampler,
            hdr,
            ldr,
        }
    }
}
