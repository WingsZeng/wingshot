use std::io::{self, Cursor, Write};

use anyhow::Context;
use chrono::Local;
use clap::Parser;
use image::{DynamicImage, ImageFormat};
use log::info;
use runtime_data::RuntimeData;
use smithay_client_toolkit::reexports::client::{globals::registry_queue_init, Connection};
use traits::{Contains, ToLocal};
use types::{Args, Config, ExitState, Monitor, Rect, SaveLocation, Selection};
use wl_clipboard_rs::copy;

mod macros;
mod runtime_data;
mod traits;
mod types;

pub mod window;

mod sctk_impls {
    mod compositor_handler;
    mod keyboard_handler;
    mod layer_shell_handler;
    mod output_handler;
    mod pointer_handler;
    mod provides_registry_state;
    mod seat_handler;
    mod shm_handler;
}
mod rendering;

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    env_logger::init();

    if let Some(image) = gui(&args).with_context(|| "Failed to initialize GUI")? {
        // Save the file if an argument for that is present
        if let Some(save_location) = &args.save {
            match save_location {
                SaveLocation::Path { path } => image.save(path),
                SaveLocation::Directory { path } => {
                    let local = Local::now();
                    image.save(
                        local
                            .format(&format!("{}/Wingshot_%d-%m-%Y_%H:%M.png", path))
                            .to_string(),
                    )
                }
            }
            .with_context(|| "Error saving image")?
        }

        // Save the selected image into the buffer
        let mut buf = Cursor::new(Vec::new());
        image
            .write_to(&mut buf, ImageFormat::Png)
            .with_context(|| "Failed to write image to buffer as PNG")?;

        let buf = buf.into_inner();

        if args.stdout {
            io::stdout()
                .lock()
                .write_all(&buf)
                .with_context(|| "Failed to write image content to stdout")?;
        }

        // Fork to serve copy requests
        if args.copy {
            match unsafe { nix::unistd::fork().with_context(|| "Failed to fork")? } {
                nix::unistd::ForkResult::Parent { .. } => {
                    info!("Forked to serve copy requests")
                }
                nix::unistd::ForkResult::Child => {
                    // Serve copy requests
                    let mut opts = copy::Options::new();
                    opts.foreground(true);
                    opts.copy(
                        copy::Source::Bytes(buf.into_boxed_slice()),
                        copy::MimeType::Autodetect,
                    )
                    .with_context(|| "Failed to serve copied image")?;
                }
            }
        }
    }

    Ok(())
}

fn gui(args: &Args) -> anyhow::Result<Option<DynamicImage>> {
    let conn = Connection::connect_to_env().with_context(|| "Could not connect to the Wayland server, make sure you run wingshot within a Wayland session!")?;

    let (globals, mut event_queue) =
        registry_queue_init(&conn).with_context(|| "Failed initialize a new event queue")?;
    let qh = event_queue.handle();
    let mut runtime_data = RuntimeData::new(&qh, &globals, args.clone())
        .with_context(|| "Failed to create runtime data")?;

    // Fetch the outputs from the compositor
    event_queue
        .roundtrip(&mut runtime_data)
        .with_context(|| "Failed to roundtrip the event queue")
        .with_context(|| "Failed to fetch the outputs from the compositor")?;
    // Has to be iterated first to get the full area size
    let sizes = runtime_data
        .output_state
        .outputs()
        .map(|output| {
            let info = runtime_data
                .output_state
                .info(&output)
                .with_context(|| "Failed to get output info")?;
            let size = info
                .logical_size
                .map(|(w, h)| (w as u32, h as u32))
                .with_context(|| "Can't determine monitor size!")?;
            let pos = info
                .logical_position
                .with_context(|| "Can't determine monitor position!")?;

            let rect = Rect {
                x: pos.0,
                y: pos.1,
                width: size.0 as i32,
                height: size.1 as i32,
            };

            // Extend the area spanning all monitors with the current monitor
            runtime_data.area.extend(&rect);
            Ok((rect, output, info))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    runtime_data.scale_factor = runtime_data.image.width() as f32 / runtime_data.area.width as f32;

    for (rect, output, info) in sizes {
        runtime_data.monitors.push(
            Monitor::new(rect, &qh, &conn, output, info, &runtime_data)
                .with_context(|| "Failed to create a monitor")?,
        )
    }

    event_queue
        .roundtrip(&mut runtime_data)
        .with_context(|| "Failed to roundtrip the event queue")?;

    loop {
        event_queue
            .blocking_dispatch(&mut runtime_data)
            .with_context(|| "Failed to dispatch events")?;
        match runtime_data.exit {
            ExitState::ExitOnly => return Ok(None),
            ExitState::ExitWithSelection(rect) => {
                let image = match runtime_data
                    .monitors
                    .into_iter()
                    .find_map(|mon| mon.rect.contains(&rect).then_some(mon))
                {
                    Some(mon) => {
                        let rect = rect.to_local(&mon.rect);
                        mon.image.crop_imm(
                            rect.x as u32,
                            rect.y as u32,
                            rect.width as u32,
                            rect.height as u32,
                        )
                    }
                    None => runtime_data.image.crop_imm(
                        (rect.x as f32 * runtime_data.scale_factor) as u32,
                        (rect.y as f32 * runtime_data.scale_factor) as u32,
                        (rect.width as f32 * runtime_data.scale_factor) as u32,
                        (rect.height as f32 * runtime_data.scale_factor) as u32,
                    ),
                };

                return Ok(Some(image));
            }
            ExitState::None => (),
        }
    }
}
