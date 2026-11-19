//! The few things only Android's Java side can answer.
//!
//! The rest of the app is pure Rust on top of `android-activity`. This exists because
//! `MANAGE_EXTERNAL_STORAGE` has no native API: it is not a dialog permission, whoever wants it
//! sends the user to a Settings page, and that page only opens through an `Intent`.

use android_activity::AndroidApp;

/// Does Android grant access to all of storage? Calls
/// `Environment.isExternalStorageManager()`.
///
/// Without it the app only reads its own folder — and, worse, **listing gives no error at all**:
/// `read_dir` works and it is `File::open` that fails. That is why the library filled up with
/// names and no game would open.
pub fn has_file_access(app: &AndroidApp) -> bool {
    fn attempt(app: &AndroidApp) -> Result<bool, jni::errors::Error> {
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }?;
        let mut env = vm.attach_current_thread()?;
        let class_id = env.find_class("android/os/Environment")?;
        env.call_static_method(class_id, "isExternalStorageManager", "()Z", &[])?
            .z()
    }
    match attempt(app) {
        Ok(has) => has,
        Err(error) => {
            log::error!("could not ask Android about the access: {error}");
            false
        }
    }
}

/// Opens the screen where the user grants access to all files.
pub fn request_file_access(app: &AndroidApp) {
    if let Err(error) = open_intent(
        app,
        "android.settings.MANAGE_APP_ALL_FILES_ACCESS_PERMISSION",
        None,
    ) {
        log::error!("could not open the permission screen: {error}");
    }
}

/// Opens an address in the device's browser. This is how the "About" tab's links get out.
pub fn open_url(app: &AndroidApp, address: &str) {
    if let Err(error) = open_intent(app, "android.intent.action.VIEW", Some(address)) {
        log::error!("could not open {address}: {error}");
    }
}

/// Fires an `Intent` with an action and, when there is one, an address.
///
/// Without an address, the Settings screen gets `package:<our name>`, which is what makes it open
/// straight on Zeebx instead of the list of all apps.
fn open_intent(
    app: &AndroidApp,
    action: &str,
    address: Option<&str>,
) -> Result<(), jni::errors::Error> {
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }?;
    let mut env = vm.attach_current_thread()?;
    let activity = unsafe { jni::objects::JObject::from_raw(app.activity_as_ptr().cast()) };

    let target = match address {
        Some(address) => address.to_string(),
        None => {
            let name = env
                .call_method(&activity, "getPackageName", "()Ljava/lang/String;", &[])?
                .l()?;
            let name: String = env.get_string(&name.into())?.into();
            format!("package:{name}")
        }
    };

    let text = env.new_string(target)?;
    let uri = env
        .call_static_method(
            "android/net/Uri",
            "parse",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[(&text).into()],
        )?
        .l()?;

    let action = env.new_string(action)?;
    let intent = env.new_object(
        "android/content/Intent",
        "(Ljava/lang/String;Landroid/net/Uri;)V",
        &[(&action).into(), (&uri).into()],
    )?;
    env.call_method(
        &activity,
        "startActivity",
        "(Landroid/content/Intent;)V",
        &[(&intent).into()],
    )?;
    Ok(())
}

/// Hides the navigation bar, and keeps it hidden.
///
/// The `WindowManager`'s `FULLSCREEN` flag removes the status bar and only that one; the
/// navigation bar belongs to the *view*, and what hides it is the root view's
/// `setSystemUiVisibility`. On a screen 960 wide it eats a whole column of the grid, and over the
/// game it covers a corner of the frame.
///
/// `IMMERSIVE_STICKY` is what makes it disappear again on its own after a swipe from the edge —
/// without it, the first touch near the corner brings the bar back for good.
///
/// It has to run on Java's main thread: touching a view from another thread is an immediate
/// exception.
pub fn hide_navigation_bar(app: &AndroidApp) {
    // `android.view.View`'s `SYSTEM_UI_FLAG_*`, added together: LAYOUT_STABLE (256),
    // LAYOUT_HIDE_NAVIGATION (512), LAYOUT_FULLSCREEN (1024), HIDE_NAVIGATION (2),
    // FULLSCREEN (4) and IMMERSIVE_STICKY (4096).
    const FLAGS: i32 = 256 | 512 | 1024 | 2 | 4 | 4096;

    let app = app.clone();
    app.clone().run_on_java_main_thread(Box::new(move || {
        if let Err(error) = attempt(&app, FLAGS) {
            log::error!("could not hide the navigation bar: {error}");
        }
    }));

    fn attempt(app: &AndroidApp, flags: i32) -> Result<(), jni::errors::Error> {
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }?;
        let mut env = vm.attach_current_thread()?;
        let activity = unsafe { jni::objects::JObject::from_raw(app.activity_as_ptr().cast()) };
        let window = env
            .call_method(&activity, "getWindow", "()Landroid/view/Window;", &[])?
            .l()?;
        let view = env
            .call_method(&window, "getDecorView", "()Landroid/view/View;", &[])?
            .l()?;
        env.call_method(
            &view,
            "setSystemUiVisibility",
            "(I)V",
            &[jni::objects::JValue::Int(flags)],
        )?;
        Ok(())
    }
}
