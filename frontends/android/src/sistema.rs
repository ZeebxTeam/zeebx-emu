//! As poucas coisas que só o Java do Android sabe responder.
//!
//! O resto do aplicativo é Rust puro sobre a `android-activity`. Isto aqui existe porque o
//! `MANAGE_EXTERNAL_STORAGE` não tem API nativa: ele não é permissão de diálogo, quem o quer
//! manda o usuário a uma página de Ajustes, e essa página só se abre por `Intent`.

use android_activity::AndroidApp;

/// O Android concede acesso a todo o armazenamento? Chama o
/// `Environment.isExternalStorageManager()`.
///
/// Sem isto o aplicativo só lê a própria pasta — e, pior, **lê sem erro nenhum na listagem**: o
/// `read_dir` funciona e o `File::open` é que falha. É por isso que a biblioteca enchia de nomes
/// e nenhum jogo abria.
pub fn tem_acesso_a_arquivos(app: &AndroidApp) -> bool {
    fn tenta(app: &AndroidApp) -> Result<bool, jni::errors::Error> {
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }?;
        let mut env = vm.attach_current_thread()?;
        let classe = env.find_class("android/os/Environment")?;
        env.call_static_method(classe, "isExternalStorageManager", "()Z", &[])?
            .z()
    }
    match tenta(app) {
        Ok(tem) => tem,
        Err(erro) => {
            log::error!("não deu para perguntar ao Android sobre o acesso: {erro}");
            false
        }
    }
}

/// Abre a tela em que o usuário concede o acesso a todos os arquivos.
pub fn pede_acesso_a_arquivos(app: &AndroidApp) {
    if let Err(erro) = abre_intencao(
        app,
        "android.settings.MANAGE_APP_ALL_FILES_ACCESS_PERMISSION",
        None,
    ) {
        log::error!("não deu para abrir a tela de permissão: {erro}");
    }
}

/// Abre um endereço no navegador do aparelho. É como os links da aba "Sobre" saem daqui.
pub fn abre_endereco(app: &AndroidApp, endereco: &str) {
    if let Err(erro) = abre_intencao(app, "android.intent.action.VIEW", Some(endereco)) {
        log::error!("não deu para abrir {endereco}: {erro}");
    }
}

/// Dispara uma `Intent` com uma ação e, quando há, um endereço.
///
/// Sem endereço, a tela de Ajustes recebe `package:<nosso nome>`, que é o que a faz abrir já
/// no Zeebx em vez da lista de todos os aplicativos.
fn abre_intencao(
    app: &AndroidApp,
    acao: &str,
    endereco: Option<&str>,
) -> Result<(), jni::errors::Error> {
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }?;
    let mut env = vm.attach_current_thread()?;
    let atividade = unsafe { jni::objects::JObject::from_raw(app.activity_as_ptr().cast()) };

    let alvo = match endereco {
        Some(endereco) => endereco.to_string(),
        None => {
            let nome = env
                .call_method(&atividade, "getPackageName", "()Ljava/lang/String;", &[])?
                .l()?;
            let nome: String = env.get_string(&nome.into())?.into();
            format!("package:{nome}")
        }
    };

    let texto = env.new_string(alvo)?;
    let uri = env
        .call_static_method(
            "android/net/Uri",
            "parse",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[(&texto).into()],
        )?
        .l()?;

    let acao = env.new_string(acao)?;
    let intencao = env.new_object(
        "android/content/Intent",
        "(Ljava/lang/String;Landroid/net/Uri;)V",
        &[(&acao).into(), (&uri).into()],
    )?;
    env.call_method(
        &atividade,
        "startActivity",
        "(Landroid/content/Intent;)V",
        &[(&intencao).into()],
    )?;
    Ok(())
}

/// Esconde a barra de navegação, e a mantém escondida.
///
/// A bandeira `FULLSCREEN` do `WindowManager` tira a barra de status e só ela; a de navegação é
/// da *view*, e quem a esconde é o `setSystemUiVisibility` da view raiz. Numa tela de 960 de
/// largura ela come uma coluna inteira da grade, e por cima do jogo ela tapa um canto do quadro.
///
/// O `IMMERSIVE_STICKY` é o que faz ela voltar a sumir sozinha depois de um deslize na borda —
/// sem ele, o primeiro toque perto do canto traz a barra de volta para sempre.
///
/// Tem de rodar na linha principal do Java: mexer numa view de outra linha é exceção na hora.
pub fn esconde_a_barra_de_navegacao(app: &AndroidApp) {
    // `SYSTEM_UI_FLAG_*` da `android.view.View`, somados: LAYOUT_STABLE (256),
    // LAYOUT_HIDE_NAVIGATION (512), LAYOUT_FULLSCREEN (1024), HIDE_NAVIGATION (2),
    // FULLSCREEN (4) e IMMERSIVE_STICKY (4096).
    const BANDEIRAS: i32 = 256 | 512 | 1024 | 2 | 4 | 4096;

    let app = app.clone();
    app.clone().run_on_java_main_thread(Box::new(move || {
        if let Err(erro) = tenta(&app, BANDEIRAS) {
            log::error!("não deu para esconder a barra de navegação: {erro}");
        }
    }));

    fn tenta(app: &AndroidApp, bandeiras: i32) -> Result<(), jni::errors::Error> {
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) }?;
        let mut env = vm.attach_current_thread()?;
        let atividade = unsafe { jni::objects::JObject::from_raw(app.activity_as_ptr().cast()) };
        let janela = env
            .call_method(&atividade, "getWindow", "()Landroid/view/Window;", &[])?
            .l()?;
        let view = env
            .call_method(&janela, "getDecorView", "()Landroid/view/View;", &[])?
            .l()?;
        env.call_method(
            &view,
            "setSystemUiVisibility",
            "(I)V",
            &[jni::objects::JValue::Int(bandeiras)],
        )?;
        Ok(())
    }
}
