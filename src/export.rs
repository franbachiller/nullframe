use std::{
    collections::VecDeque,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{Receiver, channel, sync_channel},
    },
    thread,
};

use eframe::wgpu;

use crate::{
    analysis::AudioAnalyzer,
    audio::AudioTrack,
    creator::{CanvasFormat, SnippetRange, VisualControls},
    gpu::{self, Uniforms, VisualBindings, VisualKind},
};

const FRAME_RATE: u32 = 60;

/// Frames en vuelo entre GPU y CPU. Con 3, la GPU renderiza el frame N+2
/// mientras la CPU lee el N y FFmpeg codifica los anteriores.
const PIPELINE_DEPTH: usize = 3;

/// Etapa actual de una exportación en curso.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportStage {
    Rendering,
    Encoding,
    Done,
    Failed,
}

/// Estado compartido entre el hilo de exportación y la interfaz.
#[derive(Clone, Debug)]
pub struct ExportProgress {
    pub stage: ExportStage,
    pub current_frame: u32,
    pub total_frames: u32,
    pub message: String,
    pub output: Option<PathBuf>,
}

impl ExportProgress {
    fn new(total_frames: u32) -> Self {
        Self {
            stage: ExportStage::Rendering,
            current_frame: 0,
            total_frames,
            message: "preparando…".to_owned(),
            output: None,
        }
    }

    pub fn fraction(&self) -> f32 {
        if self.total_frames == 0 {
            return 0.0;
        }
        (self.current_frame as f32 / self.total_frames as f32).clamp(0.0, 1.0)
    }

    pub fn is_finished(&self) -> bool {
        matches!(self.stage, ExportStage::Done | ExportStage::Failed)
    }
}

/// Parámetros inmutables de una exportación.
pub struct ExportSettings {
    pub track: Arc<AudioTrack>,
    pub snippet: SnippetRange,
    pub format: CanvasFormat,
    pub kind: VisualKind,
    pub controls: VisualControls,
    pub artwork_pixels: Vec<u8>,
    pub output: PathBuf,
    pub ffmpeg: PathBuf,
}

/// Handle de una exportación en ejecución. La UI consulta `progress`.
pub struct ExportJob {
    progress: Arc<Mutex<ExportProgress>>,
}

impl ExportJob {
    /// Lanza la exportación en un hilo dedicado usando el device/queue de wgpu.
    pub fn spawn(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        settings: ExportSettings,
    ) -> Self {
        let total_frames = frame_count(settings.snippet);
        let progress = Arc::new(Mutex::new(ExportProgress::new(total_frames)));
        let thread_progress = Arc::clone(&progress);

        thread::spawn(move || {
            let result = run_export(&device, &queue, &settings, &thread_progress);
            let mut progress = thread_progress.lock().unwrap();
            match result {
                Ok(()) => {
                    progress.stage = ExportStage::Done;
                    progress.message = "listo".to_owned();
                    progress.output = Some(settings.output.clone());
                }
                Err(error) => {
                    progress.stage = ExportStage::Failed;
                    progress.message = error;
                }
            }
        });

        Self { progress }
    }

    pub fn progress(&self) -> ExportProgress {
        self.progress.lock().unwrap().clone()
    }
}

fn frame_count(snippet: SnippetRange) -> u32 {
    (snippet.length() * FRAME_RATE as f32).round().max(1.0) as u32
}

/// Codificador H.264 usado para el video. Los de hardware son varias veces
/// más rápidos; x264 queda como respaldo universal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoEncoder {
    Nvenc,
    Amf,
    Qsv,
    X264,
}

impl VideoEncoder {
    const HARDWARE: [Self; 3] = [Self::Nvenc, Self::Amf, Self::Qsv];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Nvenc => "GPU NVIDIA",
            Self::Amf => "GPU AMD",
            Self::Qsv => "GPU Intel",
            Self::X264 => "CPU",
        }
    }

    const fn is_hardware(self) -> bool {
        !matches!(self, Self::X264)
    }

    /// Ajustes de calidad pensados para 1080p60 en redes: bitrate alto y
    /// perfil High, compatible con cualquier móvil.
    const fn codec_args(self) -> &'static [&'static str] {
        match self {
            Self::Nvenc => &[
                "-c:v", "h264_nvenc", "-preset", "p5", "-tune", "hq", "-rc", "vbr", "-cq", "19",
                "-b:v", "24M", "-maxrate", "36M", "-bufsize", "48M", "-profile:v", "high",
            ],
            Self::Amf => &[
                "-c:v", "h264_amf", "-usage", "transcoding", "-quality", "quality", "-rc",
                "vbr_peak", "-b:v", "24M", "-maxrate", "36M", "-profile:v", "high",
            ],
            Self::Qsv => &[
                "-c:v", "h264_qsv", "-preset", "medium", "-global_quality", "20", "-profile:v",
                "high",
            ],
            Self::X264 => &[
                "-c:v", "libx264", "-preset", "veryfast", "-crf", "17", "-profile:v", "high",
            ],
        }
    }
}

/// Etiquetas de color BT.709 rango limitado: coinciden con la conversión que
/// hace `yuv420.wgsl`, así los móviles muestran los colores exactos.
const COLOR_ARGS: [&str; 8] = [
    "-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709", "-color_range",
    "tv",
];

static DETECTED_ENCODER: OnceLock<VideoEncoder> = OnceLock::new();

/// Devuelve el mejor codificador disponible, detectándolo una sola vez.
pub fn video_encoder(ffmpeg: &Path) -> VideoEncoder {
    *DETECTED_ENCODER.get_or_init(|| {
        VideoEncoder::HARDWARE
            .into_iter()
            .find(|&encoder| probe_encoder(ffmpeg, encoder))
            .unwrap_or(VideoEncoder::X264)
    })
}

/// Detecta el codificador en segundo plano para que la primera exportación
/// no tenga que esperar.
pub fn warm_up_encoder_detection(ffmpeg: PathBuf) {
    thread::spawn(move || {
        video_encoder(&ffmpeg);
    });
}

/// Un codificador de hardware puede estar compilado en FFmpeg pero no
/// existir en la máquina, así que se prueba con una codificación real mínima.
fn probe_encoder(ffmpeg: &Path, encoder: VideoEncoder) -> bool {
    ffmpeg_command(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=black:s=1920x1080:r=60:d=0.2",
            "-pix_fmt",
            "yuv420p",
        ])
        .args(encoder.codec_args())
        .args(["-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn ffmpeg_command(ffmpeg: &Path) -> Command {
    let mut command = Command::new(ffmpeg);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

fn run_export(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    settings: &ExportSettings,
    progress: &Arc<Mutex<ExportProgress>>,
) -> Result<(), String> {
    let audio = TempFile(
        write_snippet_wav(&settings.track, settings.snippet)
            .map_err(|error| format!("No se pudo preparar el audio: {error}"))?,
    );

    let encoder = video_encoder(&settings.ffmpeg);
    match encode(device, queue, settings, &audio.0, encoder, progress) {
        // Si el driver de la GPU falla a mitad de camino, se reintenta en CPU.
        Err(_) if encoder.is_hardware() => {
            encode(device, queue, settings, &audio.0, VideoEncoder::X264, progress)
        }
        result => result,
    }
}

fn encode(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    settings: &ExportSettings,
    audio_path: &Path,
    encoder: VideoEncoder,
    progress: &Arc<Mutex<ExportProgress>>,
) -> Result<(), String> {
    let [width, height] = settings.format.export_resolution();
    let total_frames = frame_count(settings.snippet);
    set_progress(progress, ExportStage::Rendering, 0, format!("renderizando · {}", encoder.label()));

    let renderer = OffscreenRenderer::new(
        device,
        queue,
        width,
        height,
        settings.kind,
        &settings.artwork_pixels,
    );

    let mut ffmpeg = spawn_ffmpeg(settings, audio_path, encoder, width, height)
        .map_err(|error| format!("No se pudo iniciar FFmpeg: {error}"))?;
    let mut stdin = ffmpeg
        .stdin
        .take()
        .ok_or_else(|| "FFmpeg no aceptó la entrada de video.".to_owned())?;
    let stderr_reader = ffmpeg.stderr.take().map(|mut stderr| {
        thread::spawn(move || {
            let mut log = String::new();
            let _ = stderr.read_to_string(&mut log);
            log
        })
    });

    // La escritura al pipe va en su propio hilo para que la GPU no espere a FFmpeg.
    let (frame_sender, frame_receiver) = sync_channel::<Vec<u8>>(PIPELINE_DEPTH);
    let writer = thread::spawn(move || -> std::io::Result<()> {
        for frame in frame_receiver {
            stdin.write_all(&frame)?;
        }
        Ok(())
    });

    let render_result = (|| -> Result<(), String> {
        let mut analyzer = AudioAnalyzer::new();
        let mut visual_time = 0.0_f32;
        let mut in_flight = VecDeque::with_capacity(PIPELINE_DEPTH);
        let mut written = 0;
        let speed_mult = if settings.kind == VisualKind::VinylSpin {
            settings.track.bpm / 125.0
        } else {
            1.0
        };

        let mut send = |frame: Vec<u8>| -> Result<(), String> {
            frame_sender
                .send(frame)
                .map_err(|_| "FFmpeg cerró la entrada de video.".to_owned())?;
            written += 1;
            if let Ok(mut progress) = progress.lock() {
                progress.current_frame = written;
            }
            Ok(())
        };

        for frame_index in 0..total_frames {
            let position = settings.snippet.start + frame_index as f32 / FRAME_RATE as f32;
            let analysis = analyzer.analyze(&settings.track, position).clone();
            let uniforms = Uniforms::build(
                [width as f32, height as f32],
                visual_time,
                &analysis,
                settings.controls,
            );

            if in_flight.len() == PIPELINE_DEPTH {
                let pending = in_flight.pop_front().expect("pipeline lleno");
                send(renderer.read_frame(device, pending)?)?;
            }
            in_flight.push_back(renderer.submit_frame(
                device,
                queue,
                &uniforms,
                frame_index as usize % PIPELINE_DEPTH,
            ));

            visual_time += (1.0 / FRAME_RATE as f32)
                * settings.controls.speed
                * speed_mult
                * (0.55 + analysis.rms * 0.25);
        }

        while let Some(pending) = in_flight.pop_front() {
            send(renderer.read_frame(device, pending)?)?;
        }
        Ok(())
    })();

    drop(frame_sender);
    let write_result = writer
        .join()
        .unwrap_or_else(|_| Err(std::io::Error::other("el hilo de escritura falló")));

    set_progress(progress, ExportStage::Encoding, total_frames, "finalizando mp4…".to_owned());
    let status = ffmpeg
        .wait()
        .map_err(|error| format!("FFmpeg no finalizó correctamente: {error}"))?;
    let log = stderr_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();

    if !status.success() {
        let detail = log.lines().rev().find(|line| !line.trim().is_empty());
        return Err(match detail {
            Some(line) => format!("FFmpeg falló al codificar: {}", line.trim()),
            None => "FFmpeg terminó con un error durante la codificación.".to_owned(),
        });
    }
    render_result?;
    write_result.map_err(|error| format!("FFmpeg cerró la entrada de video: {error}"))
}

fn set_progress(
    progress: &Arc<Mutex<ExportProgress>>,
    stage: ExportStage,
    current_frame: u32,
    message: String,
) {
    if let Ok(mut progress) = progress.lock() {
        progress.stage = stage;
        progress.current_frame = current_frame;
        progress.message = message;
    }
}

fn spawn_ffmpeg(
    settings: &ExportSettings,
    audio_path: &Path,
    encoder: VideoEncoder,
    width: u32,
    height: u32,
) -> std::io::Result<Child> {
    ffmpeg_command(&settings.ffmpeg)
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv420p",
            "-s",
            &format!("{width}x{height}"),
            "-r",
            &FRAME_RATE.to_string(),
        ])
        .args(COLOR_ARGS)
        .args(["-i", "pipe:0", "-i"])
        .arg(audio_path)
        .args(["-map", "0:v:0", "-map", "1:a:0"])
        .args(encoder.codec_args())
        .args(["-pix_fmt", "yuv420p"])
        .args(COLOR_ARGS)
        .args([
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-shortest",
            "-movflags",
            "+faststart",
        ])
        .arg(&settings.output)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
}

/// Archivo temporal que se borra al salir de alcance, también si hay error.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Escribe el rango del snippet como WAV IEEE-float en un archivo temporal y
/// devuelve la ruta. El volumen se exporta tal cual está en el track original.
fn write_snippet_wav(track: &AudioTrack, snippet: SnippetRange) -> std::io::Result<PathBuf> {
    let channels = usize::from(track.channels);
    let total_frames = track.samples.len() / channels;
    let start_frame = ((snippet.start * track.sample_rate as f32) as usize).min(total_frames);
    let end_frame = ((snippet.end * track.sample_rate as f32) as usize).min(total_frames);
    let start_sample = start_frame * channels;
    let end_sample = (end_frame * channels).max(start_sample);
    let samples = &track.samples[start_sample..end_sample];

    let mut path = std::env::temp_dir();
    path.push(format!("nullframe_snippet_{}.wav", std::process::id()));
    let bytes = encode_wav_f32(samples, track.channels, track.sample_rate);
    std::fs::write(&path, bytes)?;
    Ok(path)
}

fn encode_wav_f32(samples: &[f32], channels: u16, sample_rate: u32) -> Vec<u8> {
    let bits_per_sample: u16 = 32;
    let block_align = channels * (bits_per_sample / 8);
    let byte_rate = sample_rate * u32::from(block_align);
    let data_len = (samples.len() * 4) as u32;

    let mut out = Vec::with_capacity(44 + samples.len() * 4);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// Convierte en la GPU una textura RGBA a YUV 4:2:0 (yuv420p) BT.709 y deja
/// el resultado en `output`, listo para enviarlo a FFmpeg sin más conversión.
struct YuvConverter {
    pipeline: wgpu::ComputePipeline,
    bind_group: wgpu::BindGroup,
    output: wgpu::Buffer,
    width: u32,
    height: u32,
}

impl YuvConverter {
    fn new(device: &wgpu::Device, source: &wgpu::TextureView, width: u32, height: u32) -> Self {
        assert!(
            width % 8 == 0 && height % 2 == 0,
            "la conversión YUV requiere ancho múltiplo de 8 y alto par"
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("yuv420"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/yuv420.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("yuv420"),
            layout: None,
            module: &shader,
            entry_point: Some("cs_main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("yuv420_output"),
            size: yuv420_size(width, height),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let dims = [width, height, 0, 0];
        let dims_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("yuv420_dims"),
            size: size_of_val(&dims) as u64,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: true,
        });
        dims_buffer
            .slice(..)
            .get_mapped_range_mut()
            .copy_from_slice(bytemuck::cast_slice(&dims));
        dims_buffer.unmap();

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("yuv420_bind_group"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: dims_buffer.as_entire_binding(),
                },
            ],
        });

        Self {
            pipeline,
            bind_group,
            output,
            width,
            height,
        }
    }

    fn encode(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("yuv420_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.dispatch_workgroups(
            (self.width / 8).div_ceil(8),
            (self.height / 2).div_ceil(8),
            1,
        );
    }
}

fn yuv420_size(width: u32, height: u32) -> u64 {
    u64::from(width) * u64::from(height) * 3 / 2
}

/// Frame enviado a la GPU cuyo resultado aún no se ha leído.
struct PendingFrame {
    slot: usize,
    submission: wgpu::SubmissionIndex,
    mapped: Receiver<Result<(), wgpu::BufferAsyncError>>,
}

/// Recursos wgpu para renderizar la visual fuera de pantalla.
///
/// El frame se renderiza en una textura que solo es render target y luego se
/// copia en la GPU a otra textura muestreable que lee la conversión YUV. Usar
/// una sola textura como render target y como fuente del compute pass deja el
/// frame en negro con algunos drivers Vulkan (verificado en AMD Radeon); la
/// copia intermedia cuesta una fracción de milisegundo y funciona en todos.
struct OffscreenRenderer {
    pipeline: wgpu::RenderPipeline,
    bindings: VisualBindings,
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    converter_source: wgpu::Texture,
    converter: YuvConverter,
    readbacks: Vec<wgpu::Buffer>,
}

impl OffscreenRenderer {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        kind: VisualKind,
        artwork_pixels: &[u8],
    ) -> Self {
        let bindings = gpu::create_visual_bindings(device, queue, artwork_pixels);
        let pipeline = gpu::create_pipeline(
            device,
            &gpu::create_pipeline_layout(device, &bindings.layout),
            gpu::EXPORT_FORMAT,
            kind.pipeline_label(),
            kind.shader_source(),
        );

        let frame_texture = |label, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: gpu::EXPORT_FORMAT,
                usage,
                view_formats: &[],
            })
        };
        let target = frame_texture(
            "export_target",
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let converter_source = frame_texture(
            "export_converter_source",
            wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let converter = YuvConverter::new(
            device,
            &converter_source.create_view(&wgpu::TextureViewDescriptor::default()),
            width,
            height,
        );

        let readbacks = (0..PIPELINE_DEPTH)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("export_readback"),
                    size: yuv420_size(width, height),
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                })
            })
            .collect();

        Self {
            pipeline,
            bindings,
            target,
            target_view,
            converter_source,
            converter,
            readbacks,
        }
    }

    /// Renderiza un frame, lo convierte a YUV y pide su lectura sin bloquear.
    fn submit_frame(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        uniforms: &Uniforms,
        slot: usize,
    ) -> PendingFrame {
        queue.write_buffer(&self.bindings.uniform_buffer, 0, bytemuck::bytes_of(uniforms));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("export_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("export_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.target_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bindings.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_texture(
            self.target.as_image_copy(),
            self.converter_source.as_image_copy(),
            self.target.size(),
        );
        self.converter.encode(&mut encoder);
        let readback = &self.readbacks[slot];
        encoder.copy_buffer_to_buffer(&self.converter.output, 0, readback, 0, None);
        let submission = queue.submit([encoder.finish()]);

        let (sender, mapped) = channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        PendingFrame {
            slot,
            submission,
            mapped,
        }
    }

    /// Espera a que la GPU termine un frame pendiente y copia sus bytes YUV.
    fn read_frame(&self, device: &wgpu::Device, pending: PendingFrame) -> Result<Vec<u8>, String> {
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(pending.submission),
                timeout: None,
            })
            .map_err(|error| format!("Fallo al esperar la GPU: {error:?}"))?;
        pending
            .mapped
            .recv()
            .map_err(|_| "La GPU no devolvió el frame.".to_owned())?
            .map_err(|error| format!("No se pudo leer el frame: {error:?}"))?;

        let readback = &self.readbacks[pending.slot];
        let bytes = readback.slice(..).get_mapped_range().to_vec();
        readback.unmap();
        Ok(bytes)
    }
}

/// Localiza el ejecutable de FFmpeg: junto al binario, en `tools/`, o en PATH.
pub fn locate_ffmpeg() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };

    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    for base in ["tools", "."] {
        let candidate = Path::new(base).join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    ffmpeg_command(Path::new(name))
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()
        .filter(|status| status.success())
        .map(|_| PathBuf::from(name))
}

#[cfg(test)]
mod tests {
    use super::{FRAME_RATE, encode_wav_f32, frame_count};
    use crate::creator::SnippetRange;
    use eframe::wgpu;

    fn headless_gpu() -> (wgpu::Device, wgpu::Queue) {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .expect("adapter");
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .expect("device")
    }

    #[test]
    fn frame_count_matches_duration() {
        let snippet = SnippetRange {
            start: 2.0,
            end: 5.0,
            loop_enabled: true,
        };
        assert_eq!(frame_count(snippet), 3 * FRAME_RATE);
    }

    /// Referencia en CPU de la conversión BT.709 rango limitado del shader.
    fn reference_yuv420(rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
        let px = |x: usize, y: usize| {
            let i = (y * width + x) * 4;
            [rgba[i], rgba[i + 1], rgba[i + 2]].map(|v| f32::from(v) / 255.0)
        };
        let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let byte = |v: f32| v.round().clamp(0.0, 255.0) as u8;

        let mut out = Vec::with_capacity(width * height * 3 / 2);
        for y in 0..height {
            for x in 0..width {
                out.push(byte(16.0 + 219.0 * luma(px(x, y))));
            }
        }
        let mut cb = Vec::new();
        let mut cr = Vec::new();
        for y in (0..height).step_by(2) {
            for x in (0..width).step_by(2) {
                let block = [px(x, y), px(x + 1, y), px(x, y + 1), px(x + 1, y + 1)];
                let avg = [0, 1, 2].map(|c| block.iter().map(|p| p[c]).sum::<f32>() / 4.0);
                let l = luma(avg);
                cb.push(byte(128.0 + 224.0 * (avg[2] - l) / 1.8556));
                cr.push(byte(128.0 + 224.0 * (avg[0] - l) / 1.5748));
            }
        }
        out.extend(cb);
        out.extend(cr);
        out
    }

    /// Verifica el compute shader YUV contra la referencia en CPU.
    /// Requiere GPU: `cargo test gpu_yuv -- --ignored`
    #[test]
    #[ignore]
    fn gpu_yuv_conversion_matches_reference() {
        let (device, queue) = headless_gpu();
        let (width, height) = (64_u32, 34_u32);
        let rgba: Vec<u8> = (0..width * height)
            .flat_map(|i| {
                let (x, y) = (i % width, i / width);
                [(x * 4) as u8, (y * 7) as u8, ((x * y) % 256) as u8, 255]
            })
            .collect();

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: crate::gpu::EXPORT_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            texture.size(),
        );
        let view = texture.create_view(&Default::default());
        let converter = super::YuvConverter::new(&device, &view, width, height);
        let size = super::yuv420_size(width, height);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        converter.encode(&mut encoder);
        encoder.copy_buffer_to_buffer(&converter.output, 0, &readback, 0, None);
        queue.submit([encoder.finish()]);
        readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let gpu = readback.slice(..).get_mapped_range().to_vec();

        let cpu = reference_yuv420(&rgba, width as usize, height as usize);
        assert_eq!(gpu.len(), cpu.len());
        let worst = gpu
            .iter()
            .zip(&cpu)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(worst <= 1, "diferencia máxima {worst}");
    }

    /// Regresión: el frame exportado debe contener la visual, no negro.
    /// Requiere GPU: `cargo test offscreen -- --ignored`
    #[test]
    #[ignore]
    fn offscreen_frame_is_not_black() {
        use crate::{analysis::AnalysisFrame, creator::VisualControls, gpu::*};
        let (device, queue) = headless_gpu();
        let (width, height) = (1080_u32, 1920_u32);
        for kind in VisualKind::ALL {
            let renderer = super::OffscreenRenderer::new(
                &device,
                &queue,
                width,
                height,
                kind,
                &create_default_artwork(ARTWORK_SIZE, ARTWORK_SIZE),
            );
            let uniforms = Uniforms::build(
                [width as f32, height as f32],
                1.0,
                &AnalysisFrame::default(),
                VisualControls::default(),
            );
            let pending = renderer.submit_frame(&device, &queue, &uniforms, 0);
            let frame = renderer.read_frame(&device, pending).expect("frame");
            let luma = &frame[..(width * height) as usize];
            let lit = luma.iter().filter(|&&y| y > 20).count();
            assert!(lit > luma.len() / 50, "{kind:?}: frame casi negro ({lit} px)");
        }
    }

    /// Benchmark de exportación completo (GPU + FFmpeg) con un track sintético.
    /// Ejecutar con: `cargo test --release export_benchmark -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn export_benchmark() {
        use super::{ExportSettings, run_export};
        use crate::{
            audio::AudioTrack,
            creator::{CanvasFormat, VisualControls},
            gpu::{VisualKind, create_default_artwork},
        };
        use std::sync::{Arc, Mutex};

        let (device, queue) = headless_gpu();

        let sample_rate = 48_000_u32;
        let seconds = 10.0_f32;
        let frames = (sample_rate as f32 * seconds) as usize;
        let mut samples = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let t = i as f32 / sample_rate as f32;
            let kick = (-(t % 0.5) * 18.0).exp() * (t * 60.0 * std::f32::consts::TAU).sin();
            let hat = ((t * 9_000.0).sin() * (t * 7_919.0).sin()) * 0.1;
            let s = (kick * 0.7 + hat) * 0.8;
            samples.push(s);
            samples.push(s);
        }
        let track = Arc::new(AudioTrack {
            name: "bench.wav".into(),
            samples: samples.into(),
            waveform: vec![],
            channels: 2,
            sample_rate,
            duration: seconds,
            bpm: 120.0,
        });

        println!("codificador: {:?}", super::video_encoder("tools/ffmpeg.exe".as_ref()));
        let out_dir = std::env::temp_dir();
        for (format, kind) in [
            (CanvasFormat::Landscape, VisualKind::IsolineVoid),
            (CanvasFormat::Portrait, VisualKind::VinylSpin),
        ] {
            let output = out_dir.join(format!("nullframe_bench_{format:?}.mp4"));
            let settings = ExportSettings {
                track: Arc::clone(&track),
                snippet: SnippetRange {
                    start: 0.0,
                    end: seconds,
                    loop_enabled: false,
                },
                format,
                kind,
                controls: VisualControls::default(),
                artwork_pixels: create_default_artwork(512, 512),
                output: output.clone(),
                ffmpeg: super::locate_ffmpeg().expect("FFmpeg no encontrado en tools/ ni en el PATH"),
            };
            let progress = Arc::new(Mutex::new(super::ExportProgress::new(0)));
            let start = std::time::Instant::now();
            run_export(&device, &queue, &settings, &progress).expect("export");
            let elapsed = start.elapsed().as_secs_f32();
            let total = seconds * FRAME_RATE as f32;
            println!(
                "{format:?}/{kind:?}: {elapsed:.2}s para {total:.0} frames ({:.1} fps, {:.2}x tiempo real) -> {}",
                total / elapsed,
                seconds / elapsed,
                output.display()
            );
        }
    }

    #[test]
    fn wav_header_is_well_formed() {
        let samples = [0.0_f32, 0.5, -0.5, 1.0];
        let wav = encode_wav_f32(&samples, 2, 48_000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(wav.len(), 44 + samples.len() * 4);
    }
}
