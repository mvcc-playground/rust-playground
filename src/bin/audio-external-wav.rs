use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

//  cargo run --bin audio-stream 5

fn main() -> Result<()> {
    // Duração em segundos (passe como primeiro argumento). Ex.: `cargo run -- 5`
    let secs: u64 = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "5".to_string())
        .parse()
        .unwrap_or(5);
    let out_dir = PathBuf::from(".tmp");
    std::fs::create_dir_all(&out_dir).context("Erro ao criar diretório de saída")?;
    let wav_out = out_dir.join("meu_audio.wav");

    // 1) Seleciona host e dispositivo de entrada padrão
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .context("Nenhum microfone padrão encontrado")?;
    let supported_config = device
        .default_input_config()
        .context("Não foi possível obter config de entrada")?;
    let sample_format = supported_config.sample_format();
    let config: cpal::StreamConfig = supported_config.into();

    // 2) Buffer compartilhado para armazenar amostras em i16
    let samples: Arc<Mutex<Vec<i16>>> = Arc::new(Mutex::new(Vec::new()));
    let samples_clone = Arc::clone(&samples);

    let err_fn = |err| eprintln!("Erro no stream de áudio: {err}");

    // 3) Cria o stream de entrada conforme o formato do dispositivo
    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            let samples_c = samples_clone;
            device.build_input_stream(
                &config,
                move |data: &[f32], _| {
                    let mut buf = samples_c.lock().unwrap();
                    for &s in data {
                        let v =
                            (s * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                        buf.push(v);
                    }
                },
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::I16 => {
            let samples_c = samples_clone;
            device.build_input_stream(
                &config,
                move |data: &[i16], _| {
                    let mut buf = samples_c.lock().unwrap();
                    buf.extend_from_slice(data);
                },
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::U16 => {
            let samples_c = samples_clone;
            device.build_input_stream(
                &config,
                move |data: &[u16], _| {
                    let mut buf = samples_c.lock().unwrap();
                    for &s in data {
                        // Converte U16 não assinado para I16 centrando em 0
                        let v = (s as i32 - i16::MAX as i32) as i16;
                        buf.push(v);
                    }
                },
                err_fn,
                None,
            )?
        }
        _ => anyhow::bail!("Formato de amostra não suportado"),
    };

    println!("Gravando por {secs} segundo(s)... Fale no microfone.");
    stream.play()?;
    std::thread::sleep(Duration::from_secs(secs));
    drop(stream); // parar a captura

    // 4) Salva o WAV final (16-bit PCM, canais e sample_rate do dispositivo) na pasta `.tmp`
    {
        let spec = hound::WavSpec {
            channels: config.channels,
            sample_rate: config.sample_rate.0,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&wav_out, spec).context("Falha ao criar WAV")?;
        let data = samples.lock().unwrap();
        for &s in data.iter() {
            writer
                .write_sample(s)
                .context("Falha ao escrever amostra WAV")?;
        }
        writer.finalize().ok();
    }

    println!("Ok! Arquivo salvo como {}", wav_out.display());

    Ok(())
}
