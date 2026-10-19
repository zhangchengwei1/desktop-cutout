use crate::{arguments::Command, batch_bench};
use serde::Serialize;
use std::{
    error::Error,
    io::{self, Write},
};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

fn print_json(value: &impl Serialize) -> Result<()> {
    let mut output = io::stdout().lock();
    serde_json::to_writer(&mut output, value)?;
    writeln!(output)?;
    Ok(())
}

fn worker<T: Send + 'static>(
    failure: &'static str,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    std::thread::spawn(work).join().map_err(|_| failure)?
}

pub fn run(command: Command) -> Result<()> {
    match command {
        Command::Info => print_json(&cutout_core::runtime_info())?,
        Command::Inspect { image } => {
            let decoded = cutout_core::image_io::decode_image(&image)?;
            let format = cutout_core::image_io::detected_format(&image)?;
            print_json(&serde_json::json!({
                "width": decoded.width(), "height": decoded.height(),
                "format": format, "pixelFormat": "rgba8-srgb-straight"
            }))?;
        }
        Command::ApplyMask {
            image,
            mask,
            output,
        } => {
            let source = cutout_core::image_io::decode_image(&image)?;
            let gray = cutout_core::image_io::decode_gray_png(&mask)?;
            let result = cutout_core::mask::apply(&source, &gray)?;
            if output == image || output == mask {
                return Err("输出路径不能是输入图片或蒙版".into());
            }
            cutout_core::image_io::save_png_noclobber(&result, &output)?;
            println!("{}", output.display());
        }
        Command::VerifyModel { model_id, model } => {
            let spec = cutout_core::model::find(&model_id).ok_or("未知模型 ID")?;
            cutout_core::model::verify_file(&model, spec)?;
            println!("已验证: {} ({})", spec.id, spec.revision);
        }
        Command::Cutout {
            runtime,
            model,
            image,
            output,
            tensor,
            runs,
        } => {
            print_json(&worker("推理工作线程异常退出", move || {
                cutout_core::inference::cutout_file(
                    &runtime,
                    &model,
                    &image,
                    &output,
                    tensor.as_deref(),
                    runs as usize,
                )
            })?)?;
        }
        Command::InferTensor {
            runtime,
            model,
            input,
            output,
        } => {
            print_json(&worker("推理工作线程异常退出", move || {
                cutout_core::inference::infer_tensor_file(&runtime, &model, &input, &output)
            })?)?;
        }
        Command::Select {
            runtime,
            encoder,
            decoder,
            image,
            output,
            groups,
        } => {
            let groups: Vec<Vec<cutout_core::sam::Point>> =
                serde_json::from_slice(&std::fs::read(groups)?)?;
            print_json(&worker("SAM 工作线程异常退出", move || {
                cutout_core::selection::select_file(
                    &runtime, &encoder, &decoder, &image, &output, &groups,
                )
            })?)?;
        }
        Command::Refine {
            runtime,
            model,
            image,
            coarse,
            output,
            edge,
        } => {
            print_json(&worker("ViTMatte 工作线程异常退出", move || {
                cutout_core::refinement::refine_file(
                    &runtime, &model, &image, &coarse, &output, edge,
                )
            })?)?;
        }
        Command::BatchBench {
            runtime,
            models,
            plan,
            output,
            zip,
            zip_during,
        } => {
            let mode = if zip {
                batch_bench::ZipMode::After
            } else if zip_during {
                batch_bench::ZipMode::During
            } else {
                batch_bench::ZipMode::None
            };
            batch_bench::run(&runtime, &models, &plan, &output, mode)?;
        }
    }
    Ok(())
}
