use std::path::Path;

use jni::JNIEnv;
use jni::objects::{JClass, JFloatArray, JString};
use jni::sys::{jfloatArray, jstring};
use parking_lot::Mutex;

use crate::Engine;

static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);

fn error(env: &mut JNIEnv<'_>, error: impl std::fmt::Display) {
    let _ = env.throw_new("java/lang/IllegalStateException", error.to_string());
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_mineshare_mobile_NativeBridge_start(
    mut env: JNIEnv,
    _class: JClass,
    directory: JString,
    name: JString,
    width: i32,
    height: i32,
) -> jstring {
    let result = (|| -> anyhow::Result<String> {
        let directory: String = env.get_string(&directory)?.into();
        let name: String = env.get_string(&name)?.into();
        anyhow::ensure!(width > 0 && height > 0, "invalid display dimensions");
        let mut engine = ENGINE.lock();
        anyhow::ensure!(engine.is_none(), "MineShare is already running");
        let next = Engine::start(Path::new(&directory), &name, width as u32, height as u32)?;
        let id = next.device_id();
        *engine = Some(next);
        Ok(id)
    })();
    match result {
        Ok(id) => match env.new_string(id) {
            Ok(s) => s.into_raw(),
            Err(e) => {
                error(&mut env, e);
                std::ptr::null_mut()
            }
        },
        Err(e) => {
            error(&mut env, e);
            std::ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_mineshare_mobile_NativeBridge_stop(_env: JNIEnv, _class: JClass) {
    // Teardown releases networking and all tasks before the next start.
    *ENGINE.lock() = None;
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_mineshare_mobile_NativeBridge_command(
    mut env: JNIEnv,
    _class: JClass,
    message: JString,
) {
    let result = (|| -> anyhow::Result<()> {
        let message: String = env.get_string(&message)?.into();
        ENGINE
            .lock()
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("MineShare is stopped"))?
            .command(&message)
    })();
    if let Err(e) = result {
        error(&mut env, e);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_mineshare_mobile_NativeBridge_poll(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    let event = ENGINE.lock().as_ref().and_then(Engine::poll);
    match event {
        Some(event) => match env.new_string(event) {
            Ok(s) => s.into_raw(),
            Err(e) => {
                error(&mut env, e);
                std::ptr::null_mut()
            }
        },
        None => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_mineshare_mobile_NativeBridge_sendPcm(
    mut env: JNIEnv,
    _class: JClass,
    samples: JFloatArray,
) {
    let result = (|| -> anyhow::Result<()> {
        let count = env.get_array_length(&samples)? as usize;
        anyhow::ensure!(count <= 15360, "audio block too large");
        let mut pcm = vec![0.0f32; count];
        env.get_float_array_region(&samples, 0, &mut pcm)?;
        if let Some(engine) = ENGINE.lock().as_ref() {
            engine.send_pcm(pcm);
        }
        Ok(())
    })();
    if let Err(e) = result {
        error(&mut env, e);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_mineshare_mobile_NativeBridge_pollAudio(
    mut env: JNIEnv,
    _class: JClass,
) -> jfloatArray {
    let frame = ENGINE.lock().as_ref().and_then(Engine::poll_audio);
    let Some(frame) = frame else {
        return std::ptr::null_mut();
    };
    match env.new_float_array(frame.len() as i32) {
        Ok(array) => match env.set_float_array_region(&array, 0, &frame) {
            Ok(()) => array.into_raw(),
            Err(e) => {
                error(&mut env, e);
                std::ptr::null_mut()
            }
        },
        Err(e) => {
            error(&mut env, e);
            std::ptr::null_mut()
        }
    }
}
