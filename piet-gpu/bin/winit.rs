use piet::kurbo::Point;
use piet::{RenderContext, Text, TextAttribute, TextLayoutBuilder};
use piet_gpu_hal::{CmdBuf, Error, ImageLayout, Instance, Session, SubmittedCmdBuf};

use piet_gpu::{test_scenes, PietGpuRenderContext, Renderer};

use clap::{App, Arg};

use winit::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};

const NUM_FRAMES: usize = 2;

const WIDTH: usize = 2048;
const HEIGHT: usize = 1536;

fn main() -> Result<(), Error> {
    let matches = App::new("piet-gpu test")
        .arg(Arg::with_name("INPUT").index(1))
        .arg(Arg::with_name("flip").short("f").long("flip"))
        .arg(
            Arg::with_name("scale")
                .short("s")
                .long("scale")
                .takes_value(true),
        )
        .get_matches();

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_inner_size(winit::dpi::LogicalSize {
            width: (WIDTH / 2) as f64,
            height: (HEIGHT / 2) as f64,
        })
        .with_resizable(false) // currently not supported
        .build(&event_loop)?;

    let (instance, surface) = Instance::new(Some(&window), Default::default())?;
    let mut info_string = "info".to_string();
    unsafe {
        let device = instance.device(surface.as_ref())?;
        let mut swapchain =
            instance.swapchain(WIDTH / 2, HEIGHT / 2, &device, surface.as_ref().unwrap())?;
        let session = Session::new(device);

        let mut current_frame = 0;
        let present_semaphores = (0..NUM_FRAMES)
            .map(|_| session.create_semaphore())
            .collect::<Result<Vec<_>, Error>>()?;
        let query_pools = (0..NUM_FRAMES)
            .map(|_| session.create_query_pool(8))
            .collect::<Result<Vec<_>, Error>>()?;
        let mut cmd_bufs: [Option<CmdBuf>; NUM_FRAMES] = Default::default();
        let mut submitted: [Option<SubmittedCmdBuf>; NUM_FRAMES] = Default::default();

        let mut renderer = Renderer::new(&session, WIDTH, HEIGHT, NUM_FRAMES)?;

        event_loop.run(move |event, _, control_flow| {
            *control_flow = ControlFlow::Poll; // `ControlFlow::Wait` if only re-render on event

            match event {
                Event::WindowEvent { event, window_id } if window_id == window.id() => {
                    match event {
                        WindowEvent::CloseRequested => {
                            *control_flow = ControlFlow::Exit;
                        }
                        _ => (),
                    }
                }
                Event::MainEventsCleared => {
                    window.request_redraw();
                }
                Event::RedrawRequested(window_id) if window_id == window.id() => {
                    let frame_idx = current_frame % NUM_FRAMES;

                    if let Some(submitted) = submitted[frame_idx].take() {
                        cmd_bufs[frame_idx] = submitted.wait().unwrap();
                        let ts = session.fetch_query_pool(&query_pools[frame_idx]).unwrap();
                        if !ts.is_empty() {
                            info_string = format!(
                                "{:.3}ms :: e:{:.3}ms|alloc:{:.3}ms|cp:{:.3}ms|bd:{:.3}ms|bin:{:.3}ms|cr:{:.3}ms|r:{:.3}ms",
                                ts[6] * 1e3,
                                ts[0] * 1e3,
                                (ts[1] - ts[0]) * 1e3,
                                (ts[2] - ts[1]) * 1e3,
                                (ts[3] - ts[2]) * 1e3,
                                (ts[4] - ts[3]) * 1e3,
                                (ts[5] - ts[4]) * 1e3,
                                (ts[6] - ts[5]) * 1e3,
                            );
                        }
                    }

                    let mut ctx = PietGpuRenderContext::new();
                    if let Some(input) = matches.value_of("INPUT") {
                        let mut scale = matches
                            .value_of("scale")
                            .map(|scale| scale.parse().unwrap())
                            .unwrap_or(8.0);
                        if matches.is_present("flip") {
                            scale = -scale;
                        }
                        test_scenes::render_svg(&mut ctx, input, scale);
                    } else {
                        test_scenes::render_anim_frame(&mut ctx, current_frame);
                    }
                    render_info_string(&mut ctx, &info_string);
                    if let Err(e) = renderer.upload_render_ctx(&mut ctx, frame_idx) {
                        println!("error in uploading: {}", e);
                    }

                    let (image_idx, acquisition_semaphore) = swapchain.next().unwrap();
                    let swap_image = swapchain.image(image_idx);
                    let query_pool = &query_pools[frame_idx];
                    let mut cmd_buf = cmd_bufs[frame_idx].take().unwrap_or_else(|| session.cmd_buf().unwrap());
                    cmd_buf.begin();
                    renderer.record(&mut cmd_buf, &query_pool, frame_idx);

                    // Image -> Swapchain
                    cmd_buf.image_barrier(
                        &swap_image,
                        ImageLayout::Undefined,
                        ImageLayout::BlitDst,
                    );
                    cmd_buf.blit_image(&renderer.image_dev, &swap_image);
                    cmd_buf.image_barrier(&swap_image, ImageLayout::BlitDst, ImageLayout::Present);
                    cmd_buf.finish();

                    submitted[frame_idx] = Some(session
                        .run_cmd_buf(
                            cmd_buf,
                            &[&acquisition_semaphore],
                            &[&present_semaphores[frame_idx]],
                        )
                        .unwrap());

                    swapchain
                        .present(image_idx, &[&present_semaphores[frame_idx]])
                        .unwrap();

                    current_frame += 1;
                }
                Event::LoopDestroyed => {
                    for cmd_buf in &mut submitted {
                        // Wait for command list submission, otherwise dropping of renderer may
                        // cause validation errors (and possibly crashes).
                        if let Some(cmd_buf) = cmd_buf.take() {
                            cmd_buf.wait().unwrap();
                        }
                    }
                }
                _ => (),
            }
        })
    }
}

fn render_info_string(rc: &mut impl RenderContext, info: &str) {
    let layout = rc
        .text()
        .new_text_layout(info.to_string())
        .default_attribute(TextAttribute::FontSize(40.0))
        .build()
        .unwrap();
    rc.draw_text(&layout, Point::new(110.0, 50.0));
}
