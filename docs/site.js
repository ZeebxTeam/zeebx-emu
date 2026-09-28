'use strict';

// A página não guarda lista de arquivos: tudo vem da API de releases do GitHub, lida aqui no
// navegador. Assim uma versão nova aparece sem ninguém mexer no site.
//
// **Por que a lista, e não `/releases/latest`:** o `latest` do GitHub ignora pré-lançamentos, e
// até a v0.3.0 todas as versões saíram marcadas assim. Com o `latest`, a página diria que não há
// versão nenhuma. "Mais recente" aqui é a publicada por último, prévia ou não.
//
// A API sem autenticação aceita 60 consultas por hora por IP. O resultado fica guardado dez
// minutos no navegador, e se a API recusar, vale o que estiver guardado, mesmo velho.

const REPO = 'ZeebxTeam/zeebx-emu';
const API = `https://api.github.com/repos/${REPO}/releases?per_page=100`;
const PAGINA_RELEASES = `https://github.com/${REPO}/releases`;
const CACHE = 'zeebx:releases:v1';
const VALIDADE_CACHE = 10 * 60 * 1000;
const CACHE_CONTRIBUIDORES = 'zeebx:contribuidores:v1';

// ---------------------------------------------------------------------------------------------
// Idiomas. O Zeebo saiu no Brasil e no México, e o emulador já fala os três.

const TEXTOS = {
  pt: {
    'nav.baixar': 'Baixar',
    'nav.versoes': 'Versões',
    'hero.titulo': 'Emulador de Zeebo de Código Aberto',
    'hero.sub': 'O Zeebx é um emulador livre do Zeebo, o console que a TecToy lançou em 2009 no Brasil e no México.',
    'hero.compat': 'Lista de compatibilidade',
    'hero.codigo': 'Código-fonte',
    'rec.carregando': 'Procurando a versão mais recente…',
    'rec.para': 'Recomendado para o seu <strong>{aparelho}</strong>',
    'rec.verTodos': 'Ver todos os arquivos',
    'rec.baixar': 'Baixar para {so}',
    'rec.outras': 'Outras opções para {so}',
    'rec.todos': 'Não é o seu aparelho? Ver todos os arquivos',
    'rec.desconhecido': 'Não reconhecemos o seu sistema. Escolha o arquivo na lista abaixo.',
    'rec.ios': 'Ainda não há versão para iPhone e iPad. Os arquivos para os outros sistemas estão abaixo.',
    'rec.sem': 'Ainda não há arquivo para {aparelho}. Veja a lista completa abaixo.',
    'rec.macQual': 'Não deu para saber o processador do seu Mac. Escolha:',
    'rec.anterior': 'A versão {nova} saiu sem este arquivo; este é da {versao}.',
    'dica.macos': 'O macOS pode dizer que o Zeebx "está danificado". Não está: a build só não é assinada pela Apple. Arraste para Aplicativos e rode no Terminal:',
    'dica.android': 'Quando o Android pedir, permita que o navegador instale apps de fontes desconhecidas.',
    'dica.windows': 'Como o instalador ainda não é assinado, o SmartScreen pode avisar na primeira vez. Clique em "Mais informações" e depois em "Executar assim mesmo".',
    'dica.linux': 'Dê permissão de execução ao AppImage e abra com dois cliques:',
    'qual.titulo': 'Qual arquivo eu baixo?',
    'qual.app': 'O programa completo, com janela, biblioteca de jogos e configuração de controles. É o que a maioria das pessoas quer.',
    'qual.libretro': 'Para quem já joga pelo RetroArch. O .zip traz o core e o .info, que vão juntos na pasta de cores.',
    'qual.headless': 'Sem interface, configurado por config.ini. Para quem monta o próprio frontend.',
    'lista.titulo': 'Todos os arquivos',
    'lista.versao': 'Versão',
    'lista.busca': 'Buscar: windows, apk, retroarch, arm64…',
    'lista.contagem': '{n} de {total} arquivos da {versao}',
    'lista.vazio': 'Nenhum arquivo bate com a busca.',
    'lista.limpar': 'Limpar filtros',
    'lista.paraVoce': 'para você',
    'lista.baixar': 'Baixar',
    'filtro.sistema': 'Sistema',
    'filtro.tipo': 'Tipo',
    'filtro.todos': 'Todos',
    'tipo.app': 'Emulador',
    'tipo.libretro': 'Core RetroArch',
    'tipo.headless': 'Headless',
    'tipo.outro': 'Outro',
    'so.outro': 'Outros',
    'arch.x64': '64 bits',
    'arch.intel': 'Intel',
    'arch.apple': 'Apple Silicon (M1 ou mais novo)',
    'desc.exe': 'Instalador',
    'desc.dmg': 'Imagem de disco: arraste para Aplicativos',
    'desc.appimage': 'Portátil: roda sem instalar',
    'desc.deb': 'Pacote para Debian, Ubuntu e derivados',
    'desc.apk': 'Aplicativo, instalação manual',
    'desc.libretro': 'Core e .info, para a pasta de cores do RetroArch',
    'desc.wasm': 'Core em WebAssembly, para o ROMBundler',
    'desc.headless': 'Sem interface, configurado por config.ini',
    'desc.outro': 'Arquivo da release',
    'notas.titulo': 'Notas da versão {versao}',
    'versoes.titulo': 'Arquivo de versões',
    'versoes.sub': 'Todas as versões publicadas, da mais nova para a mais antiga.',
    'versoes.arquivos': '{n} arquivos',
    'versoes.ver': 'Ver arquivos',
    'versoes.github': 'No GitHub',
    'contrib.titulo': 'Quem faz o Zeebx',
    'contrib.github': 'Ver no GitHub',
    'contrib.commits': '{n} commits',
    'selo.recente': 'mais recente',
    'selo.previa': 'prévia',
    'erro.titulo': 'Não deu para falar com o GitHub agora.',
    'erro.texto': 'A API do GitHub limita as consultas por hora. Os arquivos continuam na página de releases.',
    'erro.link': 'Abrir as releases no GitHub',
    'rodape': 'O Zeebx é software livre, sob a GPL-2.0-or-later. Zeebo é marca de seus donos; este projeto não tem ligação com a TecToy nem com a Qualcomm.',
  },
  en: {
    'nav.baixar': 'Download',
    'nav.versoes': 'Versions',
    'hero.titulo': 'Open Source Zeebo Emulator',
    'hero.sub': 'Zeebx is a free emulator for the Zeebo, the console TecToy launched in 2009 in Brazil and Mexico.',
    'hero.compat': 'Compatibility list',
    'hero.codigo': 'Source code',
    'rec.carregando': 'Looking for the latest version…',
    'rec.para': 'Recommended for your <strong>{aparelho}</strong>',
    'rec.verTodos': 'See all files',
    'rec.baixar': 'Download for {so}',
    'rec.outras': 'Other options for {so}',
    'rec.todos': 'Not your device? See all files',
    'rec.desconhecido': "We couldn't recognize your system. Pick a file from the list below.",
    'rec.ios': 'There is no iPhone or iPad version yet. Files for the other systems are below.',
    'rec.sem': 'There is no file for {aparelho} yet. See the full list below.',
    'rec.macQual': "We couldn't tell which processor your Mac has. Choose:",
    'rec.anterior': 'Version {nova} shipped without this file; this one is from {versao}.',
    'dica.macos': 'macOS may say Zeebx "is damaged". It isn\'t: the build just isn\'t signed by Apple. Drag it to Applications and run in Terminal:',
    'dica.android': 'When Android asks, allow your browser to install apps from unknown sources.',
    'dica.windows': 'The installer isn\'t signed yet, so SmartScreen may warn you the first time. Click "More info" and then "Run anyway".',
    'dica.linux': 'Make the AppImage executable and double-click it:',
    'qual.titulo': 'Which file do I need?',
    'qual.app': 'The full program, with a window, a game library and controller setup. This is what most people want.',
    'qual.libretro': 'For people who already play through RetroArch. The .zip has the core and its .info, which go together in the cores folder.',
    'qual.headless': 'No interface, configured through config.ini. For people building their own frontend.',
    'lista.titulo': 'All files',
    'lista.versao': 'Version',
    'lista.busca': 'Search: windows, apk, retroarch, arm64…',
    'lista.contagem': '{n} of {total} files in {versao}',
    'lista.vazio': 'No file matches your search.',
    'lista.limpar': 'Clear filters',
    'lista.paraVoce': 'for you',
    'lista.baixar': 'Download',
    'filtro.sistema': 'System',
    'filtro.tipo': 'Type',
    'filtro.todos': 'All',
    'tipo.app': 'Emulator',
    'tipo.libretro': 'RetroArch core',
    'tipo.headless': 'Headless',
    'tipo.outro': 'Other',
    'so.outro': 'Other',
    'arch.x64': '64-bit',
    'arch.intel': 'Intel',
    'arch.apple': 'Apple Silicon (M1 or newer)',
    'desc.exe': 'Installer',
    'desc.dmg': 'Disk image: drag to Applications',
    'desc.appimage': 'Portable: runs without installing',
    'desc.deb': 'Package for Debian, Ubuntu and derivatives',
    'desc.apk': 'App, manual install',
    'desc.libretro': 'Core and .info, for the RetroArch cores folder',
    'desc.wasm': 'WebAssembly core, for ROMBundler',
    'desc.headless': 'No interface, configured through config.ini',
    'desc.outro': 'Release file',
    'notas.titulo': 'Release notes for {versao}',
    'versoes.titulo': 'Version archive',
    'versoes.sub': 'Every published version, newest first.',
    'versoes.arquivos': '{n} files',
    'versoes.ver': 'See files',
    'versoes.github': 'On GitHub',
    'contrib.titulo': 'Who makes Zeebx',
    'contrib.github': 'See on GitHub',
    'contrib.commits': '{n} commits',
    'selo.recente': 'latest',
    'selo.previa': 'preview',
    'erro.titulo': "Couldn't reach GitHub right now.",
    'erro.texto': 'The GitHub API limits requests per hour. The files are still on the releases page.',
    'erro.link': 'Open releases on GitHub',
    'rodape': 'Zeebx is free software, under the GPL-2.0-or-later. Zeebo is a trademark of its owners; this project is not affiliated with TecToy or Qualcomm.',
  },
  es: {
    'nav.baixar': 'Descargar',
    'nav.versoes': 'Versiones',
    'hero.titulo': 'Emulador de Zeebo de Código Abierto',
    'hero.sub': 'Zeebx es un emulador libre de Zeebo, la consola que TecToy lanzó en 2009 en Brasil y México.',
    'hero.compat': 'Lista de compatibilidad',
    'hero.codigo': 'Código fuente',
    'rec.carregando': 'Buscando la versión más reciente…',
    'rec.para': 'Recomendado para tu <strong>{aparelho}</strong>',
    'rec.verTodos': 'Ver todos los archivos',
    'rec.baixar': 'Descargar para {so}',
    'rec.outras': 'Otras opciones para {so}',
    'rec.todos': '¿No es tu dispositivo? Ver todos los archivos',
    'rec.desconhecido': 'No reconocimos tu sistema. Elige el archivo en la lista de abajo.',
    'rec.ios': 'Todavía no hay versión para iPhone ni iPad. Los archivos para los demás sistemas están abajo.',
    'rec.sem': 'Todavía no hay archivo para {aparelho}. Mira la lista completa abajo.',
    'rec.macQual': 'No pudimos saber qué procesador tiene tu Mac. Elige:',
    'rec.anterior': 'La versión {nova} salió sin este archivo; este es de la {versao}.',
    'dica.macos': 'macOS puede decir que Zeebx "está dañado". No lo está: la build no está firmada por Apple. Arrástralo a Aplicaciones y ejecuta en la Terminal:',
    'dica.android': 'Cuando Android lo pida, permite que el navegador instale apps de fuentes desconocidas.',
    'dica.windows': 'El instalador todavía no está firmado, así que SmartScreen puede avisar la primera vez. Haz clic en "Más información" y luego en "Ejecutar de todas formas".',
    'dica.linux': 'Da permiso de ejecución al AppImage y ábrelo con doble clic:',
    'qual.titulo': '¿Qué archivo descargo?',
    'qual.app': 'El programa completo, con ventana, biblioteca de juegos y configuración de controles. Es lo que la mayoría quiere.',
    'qual.libretro': 'Para quien ya juega con RetroArch. El .zip trae el core y el .info, que van juntos en la carpeta de cores.',
    'qual.headless': 'Sin interfaz, configurado por config.ini. Para quien arma su propio frontend.',
    'lista.titulo': 'Todos los archivos',
    'lista.versao': 'Versión',
    'lista.busca': 'Buscar: windows, apk, retroarch, arm64…',
    'lista.contagem': '{n} de {total} archivos de la {versao}',
    'lista.vazio': 'Ningún archivo coincide con la búsqueda.',
    'lista.limpar': 'Limpiar filtros',
    'lista.paraVoce': 'para ti',
    'lista.baixar': 'Descargar',
    'filtro.sistema': 'Sistema',
    'filtro.tipo': 'Tipo',
    'filtro.todos': 'Todos',
    'tipo.app': 'Emulador',
    'tipo.libretro': 'Core de RetroArch',
    'tipo.headless': 'Headless',
    'tipo.outro': 'Otro',
    'so.outro': 'Otros',
    'arch.x64': '64 bits',
    'arch.intel': 'Intel',
    'arch.apple': 'Apple Silicon (M1 o más nuevo)',
    'desc.exe': 'Instalador',
    'desc.dmg': 'Imagen de disco: arrastra a Aplicaciones',
    'desc.appimage': 'Portátil: funciona sin instalar',
    'desc.deb': 'Paquete para Debian, Ubuntu y derivados',
    'desc.apk': 'Aplicación, instalación manual',
    'desc.libretro': 'Core y .info, para la carpeta de cores de RetroArch',
    'desc.wasm': 'Core en WebAssembly, para ROMBundler',
    'desc.headless': 'Sin interfaz, configurado por config.ini',
    'desc.outro': 'Archivo de la release',
    'notas.titulo': 'Notas de la versión {versao}',
    'versoes.titulo': 'Archivo de versiones',
    'versoes.sub': 'Todas las versiones publicadas, de la más nueva a la más antigua.',
    'versoes.arquivos': '{n} archivos',
    'versoes.ver': 'Ver archivos',
    'versoes.github': 'En GitHub',
    'contrib.titulo': 'Quién hace Zeebx',
    'contrib.github': 'Ver en GitHub',
    'contrib.commits': '{n} commits',
    'selo.recente': 'más reciente',
    'selo.previa': 'vista previa',
    'erro.titulo': 'No pudimos conectar con GitHub ahora.',
    'erro.texto': 'La API de GitHub limita las consultas por hora. Los archivos siguen en la página de releases.',
    'erro.link': 'Abrir las releases en GitHub',
    'rodape': 'Zeebx es software libre, bajo la GPL-2.0-or-later. Zeebo es marca de sus dueños; este proyecto no tiene relación con TecToy ni con Qualcomm.',
  },
};

const LOCALE = { pt: 'pt-BR', en: 'en', es: 'es' };

function escolherIdioma() {
  const pedido = new URLSearchParams(location.search).get('lang') || guardado('zeebx:idioma');
  if (pedido && TEXTOS[pedido]) return pedido;
  for (const l of navigator.languages || [navigator.language || '']) {
    const base = l.slice(0, 2).toLowerCase();
    if (TEXTOS[base]) return base;
  }
  return 'pt';
}

let idioma = escolherIdioma();

function t(chave, vars = {}) {
  const texto = TEXTOS[idioma][chave] ?? TEXTOS.pt[chave] ?? chave;
  return texto.replace(/\{(\w+)\}/g, (_, k) => vars[k] ?? '');
}

function traduzirPagina() {
  document.documentElement.lang = LOCALE[idioma];
  for (const el of document.querySelectorAll('[data-t]')) el.textContent = t(el.dataset.t);
  for (const el of document.querySelectorAll('[data-t-placeholder]')) el.placeholder = t(el.dataset.tPlaceholder);
  for (const el of document.querySelectorAll('[data-t-aria]')) el.setAttribute('aria-label', t(el.dataset.tAria));
  document.getElementById('idioma').value = idioma;
}

// localStorage pode lançar exceção (janela privada, cookies bloqueados). A página funciona sem ele.
function guardado(chave) {
  try { return localStorage.getItem(chave); } catch { return null; }
}

function guardar(chave, valor) {
  try { localStorage.setItem(chave, valor); } catch { /* sem armazenamento, sem cache */ }
}

// ---------------------------------------------------------------------------------------------
// Os nomes dos arquivos.
//
// Desde a v0.3.0 o `release.yml` nomeia tudo como `zeebx-<frontend>-<sistema>-<arquitetura>`, e o
// core como `zeebx_libretro-<sistema>-<arquitetura>.zip`. Até a v0.2.1 eram os nomes do
// `cargo-packager` (`Zeebx_0.2.1_aarch64.dmg`), que só diziam a arquitetura: o sistema sai da
// extensão, e eram todos do emulador com interface. Um nome que não bata com nenhum dos dois ainda
// aparece na lista, em "Outros", em vez de sumir.

const ARQUITETURAS = {
  x86_64: 'x86_64', amd64: 'x86_64', x64: 'x86_64',
  aarch64: 'arm64', arm64: 'arm64', 'arm64-v8a': 'arm64',
  'armeabi-v7a': 'armv7', armv7: 'armv7',
  wasm: 'wasm',
};

const SISTEMA_DA_EXTENSAO = { exe: 'windows', dmg: 'macos', deb: 'linux', appimage: 'linux', apk: 'android' };

function classificar(nome) {
  const n = nome.toLowerCase();
  const formato = n.split('.').pop();
  let tipo = 'outro', so = SISTEMA_DA_EXTENSAO[formato] || 'outro', arch = null, interface_ = null, m;

  if ((m = n.match(/^zeebx_libretro-(.+)\.zip$/))) {
    tipo = 'libretro';
    if (m[1] === 'wasm') { so = 'web'; arch = 'wasm'; }
    else [so, arch] = sistemaEArquitetura(m[1]);
  } else if ((m = n.match(/^zeebx-(standalone|headless)-(?:(egui|qt)-)?(.+?)(?:-setup)?\.[a-z]+$/))) {
    // O desktop sai em dois frontends, `zeebx-standalone-egui-…` e `zeebx-standalone-qt-…`; o nome
    // sem a interface é o das releases de antes da separação, que eram do egui.
    tipo = m[1] === 'standalone' ? 'app' : 'headless';
    if (tipo === 'app' && so !== 'android') interface_ = m[2] || 'egui';
    [so, arch] = sistemaEArquitetura(m[3]);
  } else if ((m = n.match(/^zeebx-android-(.+)\.apk$/))) {
    tipo = 'app'; so = 'android'; arch = ARQUITETURAS[m[1]] || m[1];
  } else if ((m = n.match(/^zeebx_[\d.]+_(.+?)(?:-setup)?\.(exe|dmg|deb|appimage)$/))) {
    tipo = 'app'; arch = ARQUITETURAS[m[1]] || m[1];
  }

  return { tipo, so, arch, formato, interface: interface_ };
}

function sistemaEArquitetura(s) {
  const i = s.indexOf('-');
  if (i < 0) return [s, null];
  const resto = s.slice(i + 1);
  return [s.slice(0, i), ARQUITETURAS[resto] || resto];
}

// ---------------------------------------------------------------------------------------------
// Rótulos e busca.

const ORDEM_SO = ['windows', 'macos', 'linux', 'android', 'web', 'outro'];
const ORDEM_TIPO = ['app', 'libretro', 'headless', 'outro'];
const ORDEM_FORMATO = ['exe', 'dmg', 'appimage', 'deb', 'apk', 'zip'];
const NOME_SO = { windows: 'Windows', macos: 'macOS', linux: 'Linux', android: 'Android', web: 'Web' };

function rotuloSo(so) { return NOME_SO[so] || t('so.outro'); }

function rotuloArch(so, arch) {
  if (!arch) return '';
  if (so === 'macos') return arch === 'arm64' ? t('arch.apple') : t('arch.intel');
  if (arch === 'x86_64') return `${t('arch.x64')} (x86_64)`;
  if (arch === 'arm64') return so === 'android' ? 'ARM64 (arm64-v8a)' : 'ARM64';
  if (arch === 'wasm') return 'WebAssembly';
  return arch;
}

function rotuloFormato(formato) {
  return formato === 'appimage' ? 'AppImage' : `.${formato}`;
}

function descricao(a) {
  if (a.tipo === 'libretro') return t(a.so === 'web' ? 'desc.wasm' : 'desc.libretro');
  if (a.tipo === 'headless') return t('desc.headless');
  if (a.tipo === 'app' && TEXTOS.pt[`desc.${a.formato}`]) {
    const desc = t(`desc.${a.formato}`);
    return a.interface ? `${desc} · ${a.interface === 'qt' ? 'Qt' : 'egui'}` : desc;
  }
  return t('desc.outro');
}

// Palavras que alguém digitaria procurando o arquivo, além das que já estão no nome.
const SINONIMOS = {
  windows: 'win windows pc',
  macos: 'mac macos osx apple',
  linux: 'linux ubuntu debian fedora arch steamdeck steam deck',
  android: 'android celular telefone phone tablet movil smartphone',
  web: 'web navegador browser rombundler',
  x86_64: 'x64 amd64 x86_64 intel amd 64',
  arm64: 'arm arm64 aarch64 m1 m2 m3 m4 snapdragon',
  wasm: 'wasm webassembly',
  app: 'emulador emulator standalone app aplicativo aplicacion interface',
  libretro: 'retroarch libretro core',
  headless: 'headless cli terminal config.ini frontend',
  exe: 'exe instalador installer setup',
  dmg: 'dmg',
  appimage: 'appimage portatil portable',
  deb: 'deb',
  apk: 'apk',
};

function semAcento(s) {
  return s.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();
}

function textoDeBusca(a) {
  return semAcento([
    a.nome, rotuloSo(a.so), rotuloArch(a.so, a.arch), t(`tipo.${a.tipo}`), descricao(a),
    SINONIMOS[a.so], SINONIMOS[a.arch], SINONIMOS[a.tipo], SINONIMOS[a.formato],
  ].filter(Boolean).join(' '));
}

// Mac vendido hoje é Apple Silicon, e ele vem primeiro; nos outros sistemas, o x86_64.
function pesoArch(a) {
  return a.arch === (a.so === 'macos' ? 'arm64' : 'x86_64') ? 0 : 1;
}

function ordenar(a, b) {
  return ORDEM_SO.indexOf(a.so) - ORDEM_SO.indexOf(b.so)
    || ORDEM_TIPO.indexOf(a.tipo) - ORDEM_TIPO.indexOf(b.tipo)
    || pesoArch(a) - pesoArch(b)
    || ORDEM_FORMATO.indexOf(a.formato) - ORDEM_FORMATO.indexOf(b.formato)
    || a.nome.localeCompare(b.nome);
}

function tamanho(bytes) {
  const mb = bytes / (1024 * 1024);
  return `${new Intl.NumberFormat(LOCALE[idioma], { maximumFractionDigits: 1 }).format(mb)} MB`;
}

function data(iso) {
  return new Intl.DateTimeFormat(LOCALE[idioma], { day: 'numeric', month: 'short', year: 'numeric' }).format(new Date(iso));
}

// ---------------------------------------------------------------------------------------------
// As releases.

async function buscarReleases() {
  let cache = null;
  try { cache = JSON.parse(guardado(CACHE)); } catch { /* cache corrompido, ignora */ }
  if (cache && Date.now() - cache.em < VALIDADE_CACHE) return cache.releases;

  try {
    const todas = [];
    let url = API;
    for (let pagina = 0; url && pagina < 10; pagina++) {
      // O `html+json` faz a API devolver as notas já em HTML (`body_html`), sanitizado pelo
      // próprio GitHub. Sem isso seria preciso carregar um conversor de Markdown.
      const resposta = await fetch(url, { headers: { Accept: 'application/vnd.github.html+json' } });
      if (!resposta.ok) throw new Error(`GitHub respondeu ${resposta.status}`);
      todas.push(...await resposta.json());
      url = proximaPagina(resposta.headers.get('Link'));
    }
    const releases = todas
      .filter((r) => !r.draft && r.published_at)
      .sort((a, b) => b.published_at.localeCompare(a.published_at))
      .map((r) => ({
        tag: r.tag_name,
        previa: r.prerelease,
        data: r.published_at,
        url: r.html_url,
        notas: r.body_html || '',
        arquivos: r.assets.map((a) => ({
          nome: a.name, bytes: a.size, url: a.browser_download_url, ...classificar(a.name),
        })),
      }));
    guardar(CACHE, JSON.stringify({ em: Date.now(), releases }));
    return releases;
  } catch (erro) {
    if (cache) return cache.releases;
    throw erro;
  }
}

// Quem aparece na aba "Contributors" do repositório. A API conta os commits da branch padrão e
// deixa de fora quem não tem conta ligada ao e-mail do commit; robôs (`type: Bot`) saem aqui.
// Uma consulta a mais por visita, com o mesmo cache das releases: se falhar, a seção só não aparece.
async function buscarContribuidores() {
  let cache = null;
  try { cache = JSON.parse(guardado(CACHE_CONTRIBUIDORES)); } catch { /* cache corrompido, ignora */ }
  if (cache && Date.now() - cache.em < VALIDADE_CACHE) return cache.pessoas;

  try {
    const resposta = await fetch(`https://api.github.com/repos/${REPO}/contributors?per_page=100`);
    if (!resposta.ok) throw new Error(`GitHub respondeu ${resposta.status}`);
    const pessoas = (await resposta.json())
      .filter((c) => c.type === 'User')
      .map((c) => ({ login: c.login, url: c.html_url, avatar: c.avatar_url, commits: c.contributions }));
    guardar(CACHE_CONTRIBUIDORES, JSON.stringify({ em: Date.now(), pessoas }));
    return pessoas;
  } catch {
    return cache ? cache.pessoas : [];
  }
}

function proximaPagina(link) {
  const m = link && link.match(/<([^>]+)>;\s*rel="next"/);
  return m ? m[1] : null;
}

// ---------------------------------------------------------------------------------------------
// O aparelho de quem visita.
//
// O sistema sai do user agent. A arquitetura é mais difícil: o Chromium entrega pelo
// `userAgentData`, e nos outros navegadores um Mac se denuncia pela placa de vídeo que o WebGL
// informa ("Apple M1" contra "Intel"). O Safari responde só "Apple GPU" em qualquer Mac, e aí a
// página não chuta: mostra os dois arquivos e pede para escolher.

async function detectarAparelho() {
  const ua = navigator.userAgent;
  const uad = navigator.userAgentData;
  const plataforma = (uad && uad.platform) || navigator.platform || '';
  let so = null, arch = null;

  if (/android/i.test(ua)) so = 'android';
  else if (/iphone|ipad|ipod/i.test(ua) || (/macintosh/i.test(ua) && navigator.maxTouchPoints > 1)) so = 'ios';
  else if (/windows/i.test(ua) || /^win/i.test(plataforma)) so = 'windows';
  else if (/mac/i.test(plataforma) || /macintosh/i.test(ua)) so = 'macos';
  else if (/cros|linux|x11/i.test(ua)) so = 'linux';

  if (uad && uad.getHighEntropyValues && so !== 'android') {
    try {
      const v = await uad.getHighEntropyValues(['architecture', 'bitness']);
      if (v.architecture === 'arm') arch = 'arm64';
      else if (v.architecture === 'x86' && v.bitness !== '32') arch = 'x86_64';
    } catch { /* o navegador recusou; seguem as outras pistas */ }
  }

  if (!arch) {
    if (so === 'android') arch = 'arm64';
    else if (so === 'macos') arch = processadorDoMac();
    else if (/aarch64|arm64/i.test(ua)) arch = 'arm64';
    else if (so === 'windows' || so === 'linux') arch = 'x86_64';
  }

  return { so, arch };
}

function processadorDoMac() {
  try {
    const gl = document.createElement('canvas').getContext('webgl');
    const ext = gl && gl.getExtension('WEBGL_debug_renderer_info');
    const placa = ext ? gl.getParameter(ext.UNMASKED_RENDERER_WEBGL) : '';
    if (/apple m\d/i.test(placa)) return 'arm64';
    if (/intel|amd|radeon|nvidia/i.test(placa)) return 'x86_64';
  } catch { /* sem WebGL */ }
  return null;
}

// O que oferecer primeiro em cada sistema, em ordem de preferência. No Linux o AppImage vem antes
// do .deb porque roda em qualquer distribuição. No Windows ARM o instalador x86_64 serve, pela
// emulação do próprio Windows, e por isso a arquitetura não é exigida ali. Linux ARM64 (um
// Raspberry Pi, por exemplo) não tem emulador com interface, só o core: ele é a última opção.
// O desktop sai em Qt, a interface principal, e em egui, a legada. Cada formato é pedido primeiro
// em Qt; a regra sem interface fica logo atrás, para as releases de antes do Qt.
const qtPrimeiro = (regra) => [{ ...regra, interface: 'qt' }, regra];

const PREFERENCIA = {
  windows: qtPrimeiro({ tipo: 'app', formato: 'exe', mesmaArch: false }),
  macos: qtPrimeiro({ tipo: 'app', formato: 'dmg', mesmaArch: true }),
  linux: [
    ...qtPrimeiro({ tipo: 'app', formato: 'appimage', mesmaArch: true }),
    ...qtPrimeiro({ tipo: 'app', formato: 'deb', mesmaArch: true }),
    { tipo: 'libretro', formato: 'zip', mesmaArch: true },
  ],
  android: [{ tipo: 'app', formato: 'apk', mesmaArch: false }],
};

function casa(a, regra, aparelho) {
  return a.so === aparelho.so && a.tipo === regra.tipo && a.formato === regra.formato
    && (!regra.interface || a.interface === regra.interface)
    && (!regra.mesmaArch || !aparelho.arch || a.arch === aparelho.arch);
}

// Procura da versão mais nova para trás: uma release sai com o que compilou, e se o job do macOS
// falhou na última, quem usa Mac ainda recebe a anterior, com o aviso de qual é. As regras ficam
// no laço de fora para que o AppImage de uma versão anterior ganhe do core da mais nova.
function recomendar(releases, aparelho) {
  const regras = PREFERENCIA[aparelho.so];
  if (!regras) return null;
  for (const regra of regras) {
    for (const release of releases) {
      const principais = release.arquivos.filter((a) => casa(a, regra, aparelho));
      if (!principais.length) continue;
      // Mac sem arquitetura conhecida: os dois .dmg. Nos outros casos, um só, preferindo a
      // arquitetura do aparelho quando houver mais de um.
      const escolhidos = aparelho.so === 'macos' && !aparelho.arch
        ? principais.sort(ordenar)
        : [principais.find((a) => a.arch === aparelho.arch) || principais[0]];
      const outras = release.arquivos
        .filter((a) => a.so === aparelho.so && !escolhidos.includes(a))
        .filter((a) => !aparelho.arch || !a.arch || a.arch === aparelho.arch || a.tipo === 'app')
        .sort(ordenar);
      return { release, escolhidos, outras };
    }
  }
  return null;
}

// ---------------------------------------------------------------------------------------------
// Desenho.

const ICONE_BAIXAR = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 4v11m0 0-4.5-4.5M12 15l4.5-4.5M5 20h14"/></svg>';

function el(tag, atributos = {}, ...filhos) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(atributos)) {
    if (v == null || v === false) continue;
    if (k === 'class') e.className = v;
    else if (k === 'html') e.innerHTML = v;
    else if (k.startsWith('on')) e.addEventListener(k.slice(2), v);
    else e.setAttribute(k, v === true ? '' : v);
  }
  for (const f of filhos.flat()) if (f != null && f !== false) e.append(f);
  return e;
}

function botaoBaixar(arquivo, texto, classe = 'botao') {
  const b = el('a', { class: classe, href: arquivo.url, rel: 'nofollow' });
  b.innerHTML = ICONE_BAIXAR;
  b.append(el('span', {}, texto));
  return b;
}

function nomeDoAparelho(aparelho) {
  const arch = aparelho.arch && aparelho.so !== 'android' ? rotuloArch(aparelho.so, aparelho.arch) : '';
  return arch ? `${rotuloSo(aparelho.so)} · ${arch}` : rotuloSo(aparelho.so);
}

function desenharRecomendado(estado) {
  const caixa = document.getElementById('recomendado');
  caixa.classList.remove('erro');
  const { releases, aparelho } = estado;
  const linkTodos = el('a', { class: 'rec-todos', href: '#baixar' }, t('rec.todos'));

  if (aparelho.so === 'ios' || !aparelho.so) {
    caixa.replaceChildren(
      el('p', { class: 'rec-rotulo' }, t(aparelho.so === 'ios' ? 'rec.ios' : 'rec.desconhecido')),
      el('a', { class: 'botao grande', href: '#baixar' }, t('rec.verTodos')),
    );
    return;
  }

  const rec = recomendar(releases, aparelho);
  if (!rec) {
    caixa.replaceChildren(
      el('p', { class: 'rec-rotulo' }, t('rec.sem', { aparelho: nomeDoAparelho(aparelho) })),
      el('a', { class: 'botao grande', href: '#baixar' }, t('rec.verTodos')),
    );
    return;
  }

  const { release, escolhidos, outras } = rec;
  const rotulo = el('p', { class: 'rec-rotulo', html: t('rec.para', { aparelho: escaparHtml(nomeDoAparelho(aparelho)) }) });
  const partes = [rotulo];

  if (escolhidos.length > 1) {
    partes.push(el('p', { class: 'rec-meta' }, t('rec.macQual')));
    partes.push(el('div', { class: 'rec-botoes' },
      escolhidos.map((a) => botaoBaixar(a, rotuloArch(a.so, a.arch), 'botao grande'))));
  } else {
    partes.push(botaoBaixar(escolhidos[0], t('rec.baixar', { so: rotuloSo(aparelho.so) }), 'botao grande'));
  }

  const principal = escolhidos[0];
  const meta = [release.tag, `${descricao(principal).split(':')[0]} (${rotuloFormato(principal.formato)})`,
    tamanho(principal.bytes), data(release.data)];
  partes.push(el('p', { class: 'rec-meta' }, meta.join(' · ')));

  if (release !== releases[0]) {
    partes.push(el('p', { class: 'rec-meta' }, t('rec.anterior', { nova: releases[0].tag, versao: release.tag })));
  }

  partes.push(dica(aparelho.so, principal));

  if (outras.length) {
    partes.push(el('div', { class: 'rec-outras' },
      t('rec.outras', { so: rotuloSo(aparelho.so) }),
      el('ul', {}, outras.map((a) => el('li', {},
        botaoBaixar(a, `${t(`tipo.${a.tipo}`)} ${rotuloFormato(a.formato)}${a.arch && a.arch !== aparelho.arch ? ` · ${a.arch}` : ''}`,
          'botao secundario pequeno')))),
    ));
  }

  partes.push(linkTodos);
  caixa.replaceChildren(...partes.filter(Boolean));
}

function dica(so, arquivo) {
  if (so === 'macos') return el('div', { class: 'dica' }, t('dica.macos'),
    el('code', {}, 'xattr -dr com.apple.quarantine "/Applications/Zeebx.app"'));
  if (so === 'linux' && arquivo.formato === 'appimage') return el('div', { class: 'dica' }, t('dica.linux'),
    el('code', {}, `chmod +x ${arquivo.nome}`));
  if (so === 'windows') return el('div', { class: 'dica' }, t('dica.windows'));
  if (so === 'android') return el('div', { class: 'dica' }, t('dica.android'));
  return null;
}

function escaparHtml(s) {
  return s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}

function releaseSelecionada(estado) {
  return estado.releases.find((r) => r.tag === estado.versao) || estado.releases[0];
}

function desenharFiltros(estado) {
  const release = releaseSelecionada(estado);
  const sistemas = ORDEM_SO.filter((so) => release.arquivos.some((a) => a.so === so));
  const tipos = ORDEM_TIPO.filter((tp) => release.arquivos.some((a) => a.tipo === tp));

  const chips = (id, valores, atual, rotulo, campo) => {
    document.getElementById(id).replaceChildren(
      ...[null, ...valores].map((v) => el('button', {
        type: 'button', class: 'chip', 'aria-pressed': String(atual === v),
        onclick: () => { estado[campo] = v; atualizar(estado); },
      }, v ? rotulo(v) : t('filtro.todos'))),
    );
  };
  chips('filtro-so', sistemas, estado.so, rotuloSo, 'so');
  chips('filtro-tipo', tipos, estado.tipo, (v) => t(`tipo.${v}`), 'tipo');
}

function desenharLista(estado) {
  const release = releaseSelecionada(estado);
  const termos = semAcento(estado.busca).split(/\s+/).filter(Boolean);
  const recomendados = new Set((recomendar([release], estado.aparelho)?.escolhidos || []).map((a) => a.nome));

  const visiveis = release.arquivos
    .filter((a) => !estado.so || a.so === estado.so)
    .filter((a) => !estado.tipo || a.tipo === estado.tipo)
    .filter((a) => { const txt = textoDeBusca(a); return termos.every((p) => txt.includes(p)); })
    .sort(ordenar);

  document.getElementById('contagem').textContent =
    t('lista.contagem', { n: visiveis.length, total: release.arquivos.length, versao: release.tag });

  const lista = document.getElementById('lista');
  if (!visiveis.length) {
    lista.replaceChildren(el('div', { class: 'cartao vazio' },
      el('p', {}, t('lista.vazio')),
      el('button', { type: 'button', class: 'botao secundario', onclick: () => limparFiltros(estado) }, t('lista.limpar'))));
    return;
  }

  const grupos = ORDEM_SO.map((so) => [so, visiveis.filter((a) => a.so === so)]).filter(([, a]) => a.length);
  lista.replaceChildren(...grupos.map(([so, arquivos]) => el('div', { class: 'grupo' },
    el('h3', {}, rotuloSo(so)),
    el('ul', { class: 'arquivos' }, arquivos.map((a) => {
      const destaque = recomendados.has(a.nome);
      return el('li', { class: destaque ? 'arquivo destaque' : 'arquivo' },
        el('div', {},
          el('div', { class: 'arquivo-titulo' },
            `${t(`tipo.${a.tipo}`)} ${rotuloFormato(a.formato)}`,
            a.arch && el('span', { class: 'selo' }, rotuloArch(a.so, a.arch)),
            destaque && el('span', { class: 'selo acento' }, t('lista.paraVoce'))),
          el('div', { class: 'arquivo-desc' }, descricao(a)),
          el('div', { class: 'arquivo-nome' }, a.nome)),
        el('span', { class: 'arquivo-tamanho' }, tamanho(a.bytes)),
        botaoBaixar(a, t('lista.baixar'), destaque ? 'botao' : 'botao secundario'));
    })),
  )));
}

function desenharNotas(estado) {
  const release = releaseSelecionada(estado);
  const notas = document.getElementById('notas');
  notas.hidden = !release.notas;
  notas.querySelector('summary').textContent = t('notas.titulo', { versao: release.tag });
  notas.querySelector('.notas-corpo').innerHTML = release.notas;
}

function desenharVersoes(estado) {
  const select = document.getElementById('versao');
  select.disabled = false;
  select.replaceChildren(...estado.releases.map((r, i) => el('option', { value: r.tag },
    [r.tag, i === 0 && `(${t('selo.recente')})`, r.previa && `· ${t('selo.previa')}`].filter(Boolean).join(' '))));
  select.value = releaseSelecionada(estado).tag;

  const atual = releaseSelecionada(estado).tag;
  document.getElementById('historico').replaceChildren(...estado.releases.map((r, i) => el('li', { class: r.tag === atual ? 'atual' : null },
    el('div', { class: 'hist-tag' }, r.tag,
      i === 0 && el('span', { class: 'selo acento' }, t('selo.recente')),
      r.previa && el('span', { class: 'selo aviso' }, t('selo.previa'))),
    el('div', { class: 'hist-info' }, `${data(r.data)} · ${t('versoes.arquivos', { n: r.arquivos.length })}`),
    el('div', { class: 'hist-acoes' },
      el('a', { href: r.url }, t('versoes.github')),
      el('button', { type: 'button', class: 'botao secundario pequeno', onclick: () => {
        estado.versao = r.tag;
        atualizar(estado);
        document.getElementById('baixar').scrollIntoView();
      } }, t('versoes.ver'))),
  )));
}

function desenharErro() {
  const caixa = document.getElementById('recomendado');
  caixa.classList.add('erro');
  caixa.replaceChildren(
    el('p', { class: 'rec-rotulo' }, el('strong', {}, t('erro.titulo'))),
    el('p', { class: 'rec-meta' }, t('erro.texto')),
    el('p', {}, el('a', { class: 'botao grande', href: PAGINA_RELEASES }, t('erro.link'))),
  );
  document.getElementById('contagem').textContent = '';
  document.getElementById('lista').replaceChildren(
    el('div', { class: 'cartao vazio' }, el('a', { href: PAGINA_RELEASES }, t('erro.link'))));
}

function desenharContribuidores(pessoas) {
  document.getElementById('contribuidores').hidden = !pessoas.length;
  document.getElementById('pessoas').replaceChildren(...pessoas.map((p) => el('li', {},
    el('a', { class: 'pessoa', href: p.url },
      // O avatar vem do GitHub em qualquer tamanho; 96 px cobre a tela de alta densidade.
      el('img', { src: `${p.avatar}${p.avatar.includes('?') ? '&' : '?'}s=96`, alt: '', width: 48, height: 48, loading: 'lazy' }),
      el('span', { class: 'pessoa-nome' }, p.login),
      el('span', { class: 'pessoa-commits' }, t('contrib.commits', { n: p.commits }))))));
}

// ---------------------------------------------------------------------------------------------
// Estado na URL, para que um link como `?os=linux&tipo=libretro` abra já filtrado.

function lerUrl() {
  const p = new URLSearchParams(location.search);
  return { versao: p.get('v'), so: p.get('os'), tipo: p.get('tipo'), busca: p.get('q') || '' };
}

function gravarUrl(estado) {
  const p = new URLSearchParams(location.search);
  const campos = {
    v: estado.versao && estado.versao !== estado.releases[0]?.tag ? estado.versao : null,
    os: estado.so, tipo: estado.tipo, q: estado.busca || null,
  };
  for (const [k, v] of Object.entries(campos)) v ? p.set(k, v) : p.delete(k);
  const busca = p.toString();
  history.replaceState(null, '', `${location.pathname}${busca ? `?${busca}` : ''}${location.hash}`);
}

function limparFiltros(estado) {
  Object.assign(estado, { so: null, tipo: null, busca: '' });
  document.getElementById('busca').value = '';
  atualizar(estado);
}

function atualizar(estado) {
  if (!estado.releases) return;
  // Um filtro que não existe na versão escolhida (Android numa release antiga) esconderia tudo.
  const release = releaseSelecionada(estado);
  if (estado.so && !release.arquivos.some((a) => a.so === estado.so)) estado.so = null;
  if (estado.tipo && !release.arquivos.some((a) => a.tipo === estado.tipo)) estado.tipo = null;
  desenharVersoes(estado);
  desenharFiltros(estado);
  desenharLista(estado);
  desenharNotas(estado);
  gravarUrl(estado);
}

function desenharTudo(estado) {
  traduzirPagina();
  desenharPessoas(estado);
  if (estado.erro) { desenharErro(); return; }
  if (!estado.releases) return;
  desenharRecomendado(estado);
  atualizar(estado);
}

function desenharPessoas(estado) {
  if (estado.pessoas) desenharContribuidores(estado.pessoas);
}

async function iniciar() {
  const estado = { ...lerUrl(), releases: null, aparelho: { so: null, arch: null }, erro: null };
  traduzirPagina();
  document.getElementById('busca').value = estado.busca;

  document.getElementById('busca').addEventListener('input', (e) => {
    estado.busca = e.target.value;
    if (estado.releases) { desenharLista(estado); gravarUrl(estado); }
  });
  document.getElementById('versao').addEventListener('change', (e) => {
    estado.versao = e.target.value;
    atualizar(estado);
  });
  document.getElementById('idioma').addEventListener('change', (e) => {
    idioma = e.target.value;
    guardar('zeebx:idioma', idioma);
    desenharTudo(estado);
  });

  // Os contribuidores não seguram o resto: chegam quando chegarem.
  buscarContribuidores().then((pessoas) => { estado.pessoas = pessoas; desenharPessoas(estado); });

  const [aparelho, releases] = await Promise.all([
    detectarAparelho(),
    buscarReleases().catch((erro) => { estado.erro = erro; return null; }),
  ]);
  estado.aparelho = aparelho;
  estado.releases = releases && releases.length ? releases : null;
  if (!estado.releases && !estado.erro) estado.erro = new Error('nenhuma release publicada');
  desenharTudo(estado);
}

iniciar();
