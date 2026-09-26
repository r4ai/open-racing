//! Per-view cloud targets. Radiance/transmittance and depth have their own history;
//! neither the window nor either XR eye can consume another view's history.

use std::collections::HashMap;

use bevy::{
    core_pipeline::{FullscreenShader, core_3d::main_transparent_pass_3d, schedule::Core3d},
    diagnostic::FrameCount,
    prelude::*,
    render::{
        Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems,
        camera::ExtractedCamera,
        diagnostic::RecordDiagnostics,
        render_asset::RenderAssets,
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery},
        texture::GpuImage,
        view::{ExtractedView, Msaa, ViewDepthTexture, ViewTarget},
    },
    shader::Source,
};

use super::{CloudParams, Clouds};

pub struct CloudRenderPlugin;

impl Plugin for CloudRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, patch_scene_taa);
        let Some(render) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render
            .init_resource::<CloudViews>()
            .add_systems(ExtractSchedule, extract)
            .add_systems(RenderStartup, init)
            .add_systems(Render, prepare.in_set(RenderSystems::PrepareBindGroups));
    }

    fn finish(&self, app: &mut App) {
        let Some(render) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        // Insert at the beginning of the existing transparent pass, retaining all
        // of its atmosphere/opaque/OIT ordering edges. This uses public schedule
        // APIs and system identity, and works with Bevy's debug names disabled.
        let mut schedules = render.world_mut().resource_mut::<Schedules>();
        let schedule = schedules.get_mut(Core3d).expect("Core3d schedule");
        let identity = IntoSystem::into_system(main_transparent_pass_3d).system_type();
        let key = schedule
            .graph()
            .systems
            .iter()
            .find(|(_, system, _)| system.system_type() == identity)
            .map(|(key, _, _)| key)
            .expect("Bevy transparent pass");
        *schedule.graph_mut().systems.get_mut(key).unwrap() =
            bevy::ecs::schedule::SystemWithAccess::new(Box::new(IntoSystem::into_system(
                draw.pipe(main_transparent_pass_3d),
            )));
    }
}

/// Retain Bevy's licensed TAA shader and change only the sky history policy. The
/// original shader still owns geometry, motion-vector dilation and edge rejection.
fn patch_scene_taa(
    server: Res<AssetServer>,
    mut shaders: ResMut<Assets<Shader>>,
    mut handle: Local<Option<Handle<Shader>>>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    let handle =
        handle.get_or_insert_with(|| server.load("embedded://bevy_anti_alias/taa/taa.wgsl"));
    if !server.is_loaded_with_dependencies(handle.id()) {
        return;
    }
    let Some(mut shader) = shaders.get_mut(handle.id()) else {
        return;
    };
    let Source::Wgsl(source) = &shader.source else {
        return;
    };
    let marker = "    // Fetch the current sample";
    assert!(
        source.contains(marker),
        "Review cloud TAA integration after a Bevy upgrade"
    );
    let replacement = r#"
    // Clouds already resolve their own camera + wind motion. A transparent sky
    // cannot supply geometry motion vectors; accumulating it again leaves trails.
    if textureLoad(depth, clamp(vec2<i32>(uv * vec2<f32>(textureDimensions(depth))), vec2<i32>(0), vec2<i32>(textureDimensions(depth))-1), 0) == 0.0 {
        let sky = textureSampleLevel(view_target, nearest_sampler, uv, 0.0);
        var sky_history = sky.rgb;
#ifdef TONEMAP
        sky_history = tonemap(sky_history);
#endif
        return Output(sky, vec4(sky_history, 1.0));
    }
    // Fetch the current sample"#;
    shader.source = Source::Wgsl(source.replace(marker, replacement).into());
    *done = true;
}

#[derive(Resource, Clone)]
struct Input {
    clouds: Clouds,
    frame: u32,
}

fn extract(
    mut commands: Commands,
    clouds: Extract<Option<Res<Clouds>>>,
    frame: Extract<Res<FrameCount>>,
) {
    if let Some(c) = clouds.as_ref() {
        commands.insert_resource(Input {
            clouds: (**c).clone(),
            frame: frame.0,
        });
    } else {
        commands.remove_resource::<Input>();
    }
}

#[derive(ShaderType, Clone, Default)]
struct Uniforms {
    params: CloudParams,
    world_from_clip: Mat4,
    previous_clip_from_world: Mat4,
    camera: Vec4,
    previous_camera: Vec4,
    resolution: Vec2,
    motion: Vec4,
}

struct Target {
    _texture: Texture,
    view: TextureView,
}
impl Target {
    fn new(device: &RenderDevice, size: UVec2, format: TextureFormat) -> Self {
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("cloud target"),
            size: Extent3d {
                width: size.x,
                height: size.y,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&TextureViewDescriptor::default());
        Self {
            _texture: texture,
            view,
        }
    }
}

struct ViewState {
    size: UVec2,
    color: Target,
    depth: Target,
    history: [Target; 2],
    history_depth: [Target; 2],
    uniform: UniformBuffer<Uniforms>,
    clip: Mat4,
    camera: Vec4,
    time: f64,
    generation: u64,
    quality: u32,
    frame: u32,
    index: usize,
    valid: bool,
    composite: CachedRenderPipelineId,
}

#[derive(Resource, Default)]
struct CloudViews(HashMap<Entity, ViewState>);

#[derive(Resource)]
struct Pipelines {
    uniforms: BindGroupLayoutDescriptor,
    density: BindGroupLayoutDescriptor,
    temporal: BindGroupLayoutDescriptor,
    composite_layout: BindGroupLayoutDescriptor,
    repeat: Sampler,
    clamp: Sampler,
    march: CachedRenderPipelineId,
    resolve: CachedRenderPipelineId,
    composites: HashMap<(TextureFormat, u32), CachedRenderPipelineId>,
    shader: Handle<Shader>,
}

fn init(
    mut commands: Commands,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
    server: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
) {
    let uniforms = BindGroupLayoutDescriptor::new(
        "cloud uniforms",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (uniform_buffer::<Uniforms>(false),),
        ),
    );
    let density = BindGroupLayoutDescriptor::new(
        "cloud density",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                texture_3d(TextureSampleType::Float { filterable: true }),
                texture_3d(TextureSampleType::Float { filterable: true }),
                texture_3d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );
    let temporal = BindGroupLayoutDescriptor::new(
        "cloud temporal",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                texture_2d(TextureSampleType::Float { filterable: false }),
                texture_2d(TextureSampleType::Float { filterable: true }),
                texture_2d(TextureSampleType::Float { filterable: false }),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );
    let composite_layout = BindGroupLayoutDescriptor::new(
        "cloud composite",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );
    let repeat = device.create_sampler(&SamplerDescriptor {
        address_mode_u: AddressMode::Repeat,
        address_mode_v: AddressMode::Repeat,
        address_mode_w: AddressMode::Repeat,
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        mipmap_filter: MipmapFilterMode::Linear,
        ..default()
    });
    let clamp = device.create_sampler(&SamplerDescriptor {
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..default()
    });
    let shader = server.load("embedded://open_racing_sky/clouds.wgsl");
    let pipeline =
        |label: &'static str, layout: &BindGroupLayoutDescriptor, define: &'static str| {
            cache.queue_render_pipeline(RenderPipelineDescriptor {
                label: Some(label.into()),
                layout: vec![uniforms.clone(), layout.clone()],
                vertex: fullscreen.to_vertex_state(),
                fragment: Some(FragmentState {
                    shader: shader.clone(),
                    shader_defs: vec![define.into()],
                    entry_point: Some("fragment".into()),
                    targets: vec![
                        Some(TextureFormat::Rgba16Float.into()),
                        Some(TextureFormat::R32Float.into()),
                    ],
                }),
                ..default()
            })
        };
    let march = pipeline("cloud_march", &density, "MARCH");
    let resolve = pipeline("cloud_resolve", &temporal, "RESOLVE");
    commands.insert_resource(Pipelines {
        uniforms,
        density,
        temporal,
        composite_layout,
        repeat,
        clamp,
        march,
        resolve,
        composites: HashMap::new(),
        shader,
    });
}

#[allow(clippy::too_many_arguments)]
fn prepare(
    input: Option<Res<Input>>,
    mut views: ResMut<CloudViews>,
    mut pipelines: ResMut<Pipelines>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cache: Res<PipelineCache>,
    fullscreen: Res<FullscreenShader>,
    cameras: Query<(Entity, &ExtractedView, &ExtractedCamera, &Msaa), With<ViewTarget>>,
) {
    let Some(input) = input else {
        views.0.clear();
        return;
    };
    if input.clouds.params.steps == 0 {
        views.0.clear();
        return;
    }
    views.0.retain(|id, _| cameras.contains(*id));
    for (entity, view, camera, msaa) in &cameras {
        let size = (view.viewport.zw() / input.clouds.divisor).max(UVec2::ONE);
        let format = view.target_format;
        let samples = msaa.samples();
        let key = (format, samples);
        if !pipelines.composites.contains_key(&key) {
            let id = cache.queue_render_pipeline(RenderPipelineDescriptor {
                label: Some("cloud_composite".into()),
                layout: vec![
                    pipelines.uniforms.clone(),
                    pipelines.composite_layout.clone(),
                ],
                vertex: fullscreen.to_vertex_state(),
                fragment: Some(FragmentState {
                    shader: pipelines.shader.clone(),
                    shader_defs: vec!["COMPOSITE".into()],
                    entry_point: Some("fragment".into()),
                    targets: vec![Some(ColorTargetState {
                        format,
                        blend: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: ColorWrites::ALL,
                    })],
                }),
                depth_stencil: Some(DepthStencilState {
                    format: TextureFormat::Depth32Float,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(CompareFunction::GreaterEqual),
                    stencil: default(),
                    bias: default(),
                }),
                multisample: MultisampleState {
                    count: samples,
                    ..default()
                },
                ..default()
            });
            pipelines.composites.insert(key, id);
        }
        let clip = view
            .clip_from_world
            .unwrap_or_else(|| view.clip_from_view * view.world_from_view.to_matrix().inverse());
        let here = view.world_from_view.translation().extend(camera.exposure);
        let recreate = views
            .0
            .get(&entity)
            .is_none_or(|v| v.size != size || v.composite != pipelines.composites[&key]);
        if recreate {
            views.0.insert(
                entity,
                ViewState {
                    size,
                    color: Target::new(&device, size, TextureFormat::Rgba16Float),
                    depth: Target::new(&device, size, TextureFormat::R32Float),
                    history: std::array::from_fn(|_| {
                        Target::new(&device, size, TextureFormat::Rgba16Float)
                    }),
                    history_depth: std::array::from_fn(|_| {
                        Target::new(&device, size, TextureFormat::R32Float)
                    }),
                    uniform: default(),
                    clip,
                    camera: here,
                    time: input.clouds.time,
                    generation: input.clouds.generation,
                    quality: input.clouds.params.steps,
                    frame: input.frame,
                    index: 0,
                    valid: false,
                    composite: pipelines.composites[&key],
                },
            );
        }
        let v = views.0.get_mut(&entity).unwrap();
        let dt = (input.clouds.time - v.time) as f32;
        let cut = v.camera.truncate().distance(here.truncate()) > 100.0
            || v.clip
                .to_cols_array()
                .iter()
                .zip(clip.to_cols_array())
                .take(12)
                .any(|(a, b)| (a - b).abs() > 0.8);
        let valid = v.valid
            && !cut
            && v.generation == input.clouds.generation
            && v.quality == input.clouds.params.steps
            && input.frame == v.frame.wrapping_add(1)
            && (0.0..4.1).contains(&dt);
        v.index = 1 - v.index;
        v.uniform.set(Uniforms {
            params: input.clouds.params,
            world_from_clip: clip.inverse(),
            previous_clip_from_world: v.clip,
            camera: here,
            previous_camera: v.camera,
            resolution: size.as_vec2(),
            motion: Vec4::new(
                dt,
                if valid { 0.9 * (-dt * 0.15).exp() } else { 0.0 },
                (input.frame % 1024) as f32,
                2.0 / (view.clip_from_view.y_axis.y * size.y as f32),
            ),
        });
        v.uniform.write_buffer(&device, &queue);
        v.clip = clip;
        v.camera = here;
        v.time = input.clouds.time;
        v.generation = input.clouds.generation;
        v.quality = input.clouds.params.steps;
        v.frame = input.frame;
        // Set only after all GPU pipelines have actually run.
        v.valid = false;
    }
}

fn attachment(view: &TextureView) -> RenderPassColorAttachment<'_> {
    RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: Operations {
            load: LoadOp::Clear(default()),
            store: StoreOp::Store,
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn draw(
    view: ViewQuery<(&ViewTarget, &ViewDepthTexture, &ExtractedCamera)>,
    input: Option<Res<Input>>,
    mut views: ResMut<CloudViews>,
    pipelines: Res<Pipelines>,
    cache: Res<PipelineCache>,
    images: Res<RenderAssets<GpuImage>>,
    mut ctx: RenderContext,
) {
    let Some(input) = input else { return };
    let Some(v) = views.0.get_mut(&view.entity()) else {
        return;
    };
    let (Some(march), Some(resolve), Some(composite)) = (
        cache.get_render_pipeline(pipelines.march),
        cache.get_render_pipeline(pipelines.resolve),
        cache.get_render_pipeline(v.composite),
    ) else {
        return;
    };
    let m = &input.clouds;
    let (Some(map), Some(shape), Some(detail), Some(field)) = (
        images.get(&m.cloud_map),
        images.get(&m.shape),
        images.get(&m.detail),
        images.get(&m.field),
    ) else {
        return;
    };
    let device = ctx.render_device();
    let uniform = device.create_bind_group(
        "cloud uniforms",
        &cache.get_bind_group_layout(&pipelines.uniforms),
        &BindGroupEntries::sequential((v.uniform.binding().unwrap(),)),
    );
    let density = device.create_bind_group(
        "cloud density",
        &cache.get_bind_group_layout(&pipelines.density),
        &BindGroupEntries::sequential((
            &map.texture_view,
            &shape.texture_view,
            &detail.texture_view,
            &field.texture_view,
            &pipelines.repeat,
        )),
    );
    let temporal = device.create_bind_group(
        "cloud temporal",
        &cache.get_bind_group_layout(&pipelines.temporal),
        &BindGroupEntries::sequential((
            &v.color.view,
            &v.depth.view,
            &v.history[1 - v.index].view,
            &v.history_depth[1 - v.index].view,
            &pipelines.clamp,
        )),
    );
    let composite_bind = device.create_bind_group(
        "cloud composite",
        &cache.get_bind_group_layout(&pipelines.composite_layout),
        &BindGroupEntries::sequential((&v.history[v.index].view, &pipelines.clamp)),
    );
    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    for (name, pipeline, bind, color, depth) in [
        ("cloud_march", march, &density, &v.color.view, &v.depth.view),
        (
            "cloud_resolve",
            resolve,
            &temporal,
            &v.history[v.index].view,
            &v.history_depth[v.index].view,
        ),
    ] {
        let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some(name),
            color_attachments: &[Some(attachment(color)), Some(attachment(depth))],
            ..default()
        });
        let span = diagnostics.pass_span(&mut pass, name);
        pass.set_render_pipeline(pipeline);
        pass.set_bind_group(0, &uniform, &[]);
        pass.set_bind_group(1, bind, &[]);
        pass.draw(0..3, 0..1);
        span.end(&mut pass);
    }
    let (target, depth, camera) = view.into_inner();
    {
        let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("cloud_composite"),
            color_attachments: &[Some(target.get_color_attachment())],
            depth_stencil_attachment: Some(depth.get_attachment(StoreOp::Store)),
            ..default()
        });
        let span = diagnostics.pass_span(&mut pass, "cloud_composite");
        if let Some(viewport) = &camera.viewport {
            pass.set_camera_viewport(viewport);
        }
        pass.set_render_pipeline(composite);
        pass.set_bind_group(0, &uniform, &[]);
        pass.set_bind_group(1, &composite_bind, &[]);
        pass.draw(0..3, 0..1);
        span.end(&mut pass);
    }
    v.valid = true;
}
