# Plano Libretro — Zeebx

> Estado: implementação em validação contínua. O core Libretro já está implementado e passa a ABI;
> as tabelas históricas deste plano preservam as decisões originais.
>
> **Estado medido em 2026-09-22:** Linux AArch64 é o alvo de distribuição para R36S/R35S/RGB20S
> (ArkOS, AeolusUX, dArkOS e dArkOSen) e RG40XX-H (muOS Pixie ou mais novo). Nessa arquitetura o
> core pede GLES 3 para hardware e recua para software quando o frontend não oferece o contexto.
> Save state versionado, `.7z` e renderização hardware estão implementados. Ver
> `docs/libretro/HANDHELDS-ARM64.md`.
>
> Branch: `feat/libretro-core`, baseada em `upstream/development` no commit `6c74975` (`v0.2.1`).

## Objetivo

Entregar o Zeebx como um core Libretro para Linux **x86_64** e **AArch64**, inicialmente para
RetroArch. O core recebe um jogo Zeebo (`.mod`, `.zip`, `.7z`), executa o
applet BREW, entrega vídeo, áudio e input ao frontend, e mantém conteúdo, saves e dados do
"aparelho" em locais separados.

O primeiro alvo é um core software portátil. Janela nativa, OpenGL de host, CPAL, gamepads do
host e UI desktop não fazem parte do core.

## Estado atual medido

O projeto é um binário Rust único (`src/main.rs`), sem alvo `lib`.

Peças que podem ser reutilizadas:

- `src/session.rs::Session`: ciclo de vida de módulo/applet, Dynarmic, timers, callbacks,
  framebuffer e estado de entrada;
- `src/input/mod.rs::Pad`: estado dos botões/eixos do Z-Pad;
- `src/audio/mod.rs::Mixer`: mistura de vozes e streams; `Mixer::silent(rate)` serve a hosts
  sem dispositivo de áudio;
- `src/video/display.rs::Framebuffer`: a tela atual é 640×480 RGB565;
- `src/loader/archive.rs`: seleção segura do `.mod` em ZIP e extração de árvore inteira;
- `src/brew/vfs.rs::Vfs`: semântica BREW de `fs:/`, `fs:/~/` e caminhos relativos.

Acoplamentos a remover do caminho Libretro:

- `Session::start_with()` recebe `Option<Arc<eframe::glow::Context>>`;
- `Machine::usa_placa()` depende de `eframe::glow`;
- `Session` usa `ui::library` e `ui::settings::ZWheel`;
- `loader::archive`, widgets e SQL usam `ui::settings::config_dir()`;
- dependências desktop atuais incluem `eframe`, `glutin`, `minifb`, `rfd`, `gilrs`, `cpal` e
  Discord.

O `Session::step(Duration, speed_limit)` também **não** é a unidade correta para
`retro_run()`: ele mede/limita tempo real com `Instant`, recupera atraso e encerra por orçamento
de parede. Um core Libretro precisa de passo virtual determinístico.

## Decisões de MVP

| Assunto | Decisão |
|---|---|
| CPU | Dynarmic, como a `Session` atual |
| Vídeo | software RGB565, 640×480, 4:3 |
| Áudio | estéreo PCM16, 44 100 Hz; a quantidade por `retro_run` sai do tempo **virtual** decorrido |
| Entrada | até duas portas RetroPad → Z-Pad |
| Conteúdo inicial | `.mod` e `.zip` |
| `.7z` | feito: decodificador embutido, limites iguais aos do zip e prova com jogo real |
| Renderização HW | fora do MVP |
| Save states | fora do MVP; API retorna sem suporte |
| Core Options | nenhuma até existir opção funcional real |
| Dados persistentes | pasta `zeebx/` sob diretório de saves do frontend |
| Rede do guest | desligada por padrão; nenhuma conexão implícita do frontend |

## Arquitetura proposta

```text
src/lib.rs                         motor compartilhado (`zeebx` rlib)
src/main.rs                        CLI e frontend desktop
src/session.rs                     Session desktop + driver determinístico Libretro
src/storage.rs                     raízes persistentes, identidade e layout de conteúdo
src/loader/archive.rs              ZIP/7z, validação, cache e extração atômica
src/brew/vfs.rs                    conteúdo read-only + overlay de saves + aparelho
frontends/libretro/Cargo.toml      pacote/alvo independente `zeebx-libretro`
frontends/libretro/src/lib.rs      estado do core e exports C ABI (`cdylib`)
frontends/libretro/src/bindings.rs bindings gerados e versionados de libretro.h
frontends/libretro/include/        `libretro.h` oficial fixado em revisão conhecida
frontends/libretro/zeebx_libretro.info
                                  metadados distribuídos do core
docs/libretro/LIBRETRO_PLAN.md     este plano
```

A raiz torna-se workspace Cargo. O pacote raiz `zeebx` expõe o motor como `rlib` e mantém o
binário desktop; `frontends/libretro` é outro pacote, depende de `zeebx` por path e é o único que
produz `cdylib`. Isto impede dependências/exports Libretro de vazarem para a CLI e deixa o target
visível numa pasta própria.

### Features e workspace Cargo

Esboço:

```toml
# Cargo.toml raiz
[workspace]
members = [".", "frontends/libretro"]
resolver = "2"

[package]
name = "zeebx"

[features]
default = ["desktop"]
desktop = ["dep:eframe", "dep:glutin", "dep:minifb", "dep:rfd", "dep:gilrs", "dep:cpal"]

# frontends/libretro/Cargo.toml
[package]
name = "zeebx-libretro"

[lib]
name = "zeebx_libretro"
crate-type = ["cdylib"]

[dependencies]
zeebx = { path = "../..", default-features = false }
```

O build do core deve funcionar isoladamente:

```bash
cargo build --locked --release -p zeebx-libretro
```

Não basta o core não *usar* dependências desktop: elas precisam ser opcionais no motor e os
módulos que as importam precisam estar atrás de `cfg(feature = "desktop")`.

## Driver de frame determinístico

Criar caminho específico, por exemplo:

```rust
pub fn Session::run_libretro_frame(&mut self) -> FrameResult;
```

Ele deve:

1. não consultar `Instant` nem aplicar freio de relógio de parede;
2. entregar o `EVT_APP_START` na primeira execução, depois que áudio e input estiverem prontos;
3. executar a volta BREW: `advance`, sinais, callbacks, threads, timers e apresentações;
4. respeitar o tempo virtual/vblank do emulador;
5. guardar a última tela e telas intermediárias de `IDISPLAY_Update`;
6. retornar estado de parada sem pânico;
7. informar progresso de tempo virtual para o adaptador de áudio.

`Session::step()` permanece para a UI desktop. Implementar `retro_run()` chamando
`step(Duration::from_millis(16), ...)` é proibido: produziria timing, quantidade de áudio e
progresso de CPU dependentes do desempenho do host.

Antes de implementar a ABI, criar testes que executem número fixo de frames duas vezes e comparem
relógio virtual, estado de entrada e checksum do framebuffer.

## Contrato Libretro

Vendorizar `libretro.h` oficial numa revisão fixada. Gerar bindings Rust uma vez e versionar o
arquivo gerado, de modo que o build normal não dependa de `libclang`. Validar layouts/offsets nos
alvos x86_64 e AArch64.

Todo export deve usar ABI C e não deixar `panic` atravessar a fronteira:

```rust
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_...()
```

Entradas públicas precisam de fronteira de erro/pânico segura e log pelo frontend quando a
interface de log estiver disponível.

### Exports base obrigatórios

| Grupo | Funções |
|---|---|
| Callbacks | `retro_set_environment`, `retro_set_video_refresh`, `retro_set_audio_sample`, `retro_set_audio_sample_batch`, `retro_set_input_poll`, `retro_set_input_state` |
| Ciclo | `retro_init`, `retro_deinit`, `retro_api_version`, `retro_get_system_info`, `retro_get_system_av_info` |
| Execução | `retro_set_controller_port_device`, `retro_reset`, `retro_run` |
| Estado | `retro_serialize_size`, `retro_serialize`, `retro_unserialize` |
| Cheats | `retro_cheat_reset`, `retro_cheat_set` |
| Conteúdo | `retro_load_game`, `retro_load_game_special`, `retro_unload_game` |
| Região/memória | `retro_get_region`, `retro_get_memory_data`, `retro_get_memory_size` |

Mesmo APIs sem recurso precisam existir.

### Comportamento MVP de APIs sem recurso

| API | Resultado |
|---|---|
| `retro_load_game_special` | `false`; não há subsistemas |
| `retro_cheat_reset`/`retro_cheat_set` | no-op |
| `retro_serialize_size` | `0` |
| `retro_serialize`/`retro_unserialize` | `false` |
| `retro_get_memory_data` | `NULL` |
| `retro_get_memory_size` | `0` |
| `retro_get_region` | `RETRO_REGION_NTSC` |

`retro_reset()` não deve ser no-op. Enquanto não existir reset interno seguro, ele recria sessão a
partir do caminho e configuração original, preservando saves e dados persistentes.

`retro_unload_game()` deve destruir sessão, mixer, buffers e conteúdo temporário. `retro_deinit()`
também deve limpar estado se o frontend chamou a sequência incompleta.

### Ordem do ciclo de vida

```text
retro_set_environment
retro_init
retro_set_video_refresh / audio / input setters
retro_load_game
retro_get_system_av_info
retro_run*
retro_unload_game
retro_deinit
```

`retro_set_environment()` é chamado antes de `retro_init()`. Os ponteiros/string de
`retro_get_system_info()` e descritores devem ter vida estática. `retro_load_game()` deve recusar
ponteiro ou caminho nulo e limpar sessão anterior caso o frontend viole o fluxo.

## Informação de sistema e conteúdo

`retro_get_system_info()` deve anunciar:

```text
library_name      = "Zeebx"
library_version   = versão do Cargo
valid_extensions  = "mod|zip|7z"
need_fullpath     = true
block_extract     = true
supports_no_game  = false
```

`.mif` não é conteúdo inicializável. É manifesto lateral que identifica o applet de um `.mod`.

`block_extract=true` é obrigatório para ZIP/7z: o core precisa receber o arquivo compactado
inteiro e selecionar o módulo correto, preservando `.mif` e todos os recursos vizinhos. Não usar
`game->data`/`game->size` no caminho com `need_fullpath=true`.

`need_fullpath` não autoriza assumir que `game->path` é caminho POSIX acessível por `std::fs`: um
frontend pode fornecer caminho VFS/SAF. O core precisa obter VFS antes do load e ler conteúdo,
cache, saves e system directory pelo adaptador VFS.

Ao carregar, negociar `RETRO_ENVIRONMENT_SET_PIXEL_FORMAT` para `RETRO_PIXEL_FORMAT_RGB565`.
Recusar carregamento se o frontend não aceitar o formato.

## Vídeo

Configuração inicial de `retro_system_av_info`:

```text
base_width   = 640
base_height  = 480
max_width    = 640
max_height   = 480
aspect_ratio = 4.0 / 3.0
fps          = 60.0
sample_rate  = 44100.0
```

Cada `retro_run()` deve chamar o callback de vídeo. Se o jogo não produziu nova tela, enviar o
último framebuffer é a regra MVP mais simples. `NULL` para duplicar frame só deve ser usado após
negociar explicitamente essa capacidade do frontend.

Para RGB565:

```text
width = 640
height = 480
pitch = 1280
```

Se resolução ou geometria passar a variar, chamar `RETRO_ENVIRONMENT_SET_GEOMETRY`.

### Renderização hardware futura

Não usar Glutin/Eframe no core. Uma variante GL real exigirá:

- `RETRO_ENVIRONMENT_SET_HW_RENDER`;
- callbacks `context_reset` e `context_destroy`;
- recriação de todo recurso de GL após perda de contexto;
- funções via `get_proc_address` do frontend;
- framebuffer do frontend e `RETRO_HW_FRAME_BUFFER_VALID`.

Isto é uma segunda rota de vídeo, não uma opção trivial sobre o MVP software.

## Áudio

Libretro espera PCM16 estéreo intercalado no callback batch. O `Mixer` atual gera `f32`; o
adaptador deve aplicar clamp e converter para `i16`.

Configuração MVP:

```text
44 100 Hz, estéreo; a cada retro_run o core entrega (ms virtuais decorridos x 44,1) amostras
```

Fluxo:

1. criar `Mixer::silent(44_100)` em `retro_load_game()`;
2. conectá-lo à sessão antes do primeiro `EVT_APP_START`;
3. renderizar a quantidade associada ao passo virtual;
4. chamar `retro_audio_sample_batch`;
5. tratar retorno curto em frames com fila limitada e política documentada de descarte;
6. nunca bloquear `retro_run()` esperando áudio.

Não usar callback assíncrono de áudio do ambiente. Vídeo, áudio e input precisam sair da thread
síncrona normal do core.

## Entrada e controladores

Registrar:

```text
RETRO_ENVIRONMENT_SET_CONTROLLER_INFO
RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS
```

Porta Libretro 0 mapeia porta Zeebo 0; porta 1 mapeia porta Zeebo 1. O aparelho configurado antes
do load deve entrar em `Session::start_software()`, pois jogos podem enumerar HID já no
`EVT_APP_START`.

Mapeamento inicial:

| RetroPad | Zeebo |
|---|---|
| D-pad | `input::DPAD` |
| Y | `b1` |
| B | `b2` |
| X | `b3` |
| A | `b4` |
| L | `zl` |
| R | `zr` |
| Start | `start` |
| Select | `back`/HOME |
| analógico esquerdo X/Y | `Pad::axes[0]`/`[1]` |
| analógico direito X/Y | `Pad::axes[2]`/`[3]`, quando aplicável |

Em cada `retro_run()`:

1. chamar `input_poll()` pelo menos uma vez;
2. consultar joypad e analógicos das portas ativas;
3. normalizar valores `i16` para o curso de `Pad::set_axis()` (`-128..=128`);
4. aplicar os dois `Pad` antes de executar guest.

Input bitmask é otimização futura. Poll individual é válido e mais simples para o primeiro core.

### Aparelhos Zeebo por porta

O emulador já distingue quatro aparelhos que o guest pode enumerar por USB/HID:

| Aparelho Zeebo | Identidade/efeito atual | Fonte Libretro |
|---|---|---|
| Dragon (`Controle`) | joystick `1EAA:0135` | RetroPad padrão |
| Z-Pad | joystick `1A5C:3033`; botões/eixos iguais ao Dragon | subtipo RetroPad `Z-Pad` |
| Boomerang | receptor compartilhado, botões limitados e acelerômetro | subtipo RetroPad `Boomerang` + sensor/fallback |
| Teclado USB | enumeração HID de teclado e eventos AVK | subtipo `Teclado USB` + RetroKeyboard |

Registrar `RETRO_ENVIRONMENT_SET_CONTROLLER_INFO` para as duas portas. Cada lista deve conter
`Nenhum`, `Dragon`, `Z-Pad`, `Boomerang` e `Teclado USB`. Os três primeiros controles de jogo
podem ser subclasses de `RETRO_DEVICE_JOYPAD` usando `RETRO_DEVICE_SUBCLASS`; o core deve, como a
spec pede, continuar fazendo poll do tipo-base (`RETRO_DEVICE_JOYPAD`/`RETRO_DEVICE_ANALOG`) e
usar o subtipo apenas para definir o aparelho que o guest enxerga. `retro_set_controller_port_device`
atualiza `Machine::set_portas()` e reapresenta input descriptors; para títulos que enumeram HID no
boot, documentar reset obrigatório após trocar aparelho.

Boomerang não é um Z-Pad completo. Expor somente D-pad, `b1`, `b2` e HOME nos descritores dele,
pois estes são os bits que o receptor físico entrega. O guest já recebe relatório específico com
contador, alternância de jogador e sinal periódico de posição; preservar esse protocolo em vez de
transformar aceleração em eixos de Z-Pad.

### Movimento e Boomerang

O standalone hoje lê Wii Remote e IMUs de controles diretamente de `/dev/input` em Linux. O core
Libretro **não deve** reutilizar esses leitores: o frontend é dono de dispositivos de host, e abrir
um Wii Remote/IMU por fora falha em sandbox, duplica input e só funcionaria em parte dos sistemas.

No `retro_load_game()`, quando uma porta está configurada como Boomerang, negociar
`RETRO_ENVIRONMENT_GET_SENSOR_INTERFACE`. Se existir, chamar `set_sensor_state(porta,
RETRO_SENSOR_ACCELEROMETER_ENABLE, 100)` e, a cada `retro_run()`, ler
`ACCELEROMETER_X/Y/Z` para alimentar `Session::set_port_motion(porta, aceleracao)`. Os 100 Hz
pedidos combinam com o ritmo do receptor emulado; a máquina pode continuar emitindo os pacotes no
ritmo virtual mesmo que o frontend só atualize a amostra em 60 Hz.

A API de sensor Libretro é experimental, mas a convenção oficial é aceleração em **g**, incluindo
gravidade: parado, aproximadamente `[0, 0, 1]`; +X é direita, +Y é cima e +Z aponta ao usuário
visto de frente. A orientação física do controle e o referencial do Boomerang continuam distintos.
Portanto o adaptador precisa:

- manter calibração por porta em `<save-dir>/zeebx/input.json`;
- normalizar a leitura parada para `[0, 0, 1] g`;
- aplicar a rotação conhecida do protocolo Boomerang e orientação configurável, inclusive inversão
de eixos;
- usar último valor válido, não zero, entre leituras;
- desabilitar sensor em `retro_unload_game()`;
- informar claramente quando o frontend não tem sensor ou recusa habilitá-lo.

Um controle/Wii Remote conectado ao host só fornecerá movimento se o **frontend Libretro** o expor
por essa interface. Isso deve ser testado em RetroArch por plataforma; não é correto prometer
suporte universal a Wii Remote apenas porque o standalone Linux o lê.

Como fallback portátil, oferecer opção real por porta somente depois do MVP:

```text
zeebx_boomerang_p1_source = sensor | analog_right | static
zeebx_boomerang_p2_source = sensor | analog_right | static
```

`analog_right` converte o analógico direito em inclinação X/Y e conserva gravidade Z; `static`
entrega `[0, 0, 1]`. Não habilitar essa opção até calibrar e testar jogos Boomerang. Giroscópio não
é necessário ao protocolo conhecido; não pedir `RETRO_SENSOR_GYROSCOPE_*` sem medida que o use.

### Teclado USB e eventos BREW

`IHID` não cobre todas as entradas BREW. A Z-Wheel, em particular, pode esperar `AVK_0` ou
`AVK_CLR`; só entregar estado de Z-Pad pode deixá-la sem navegação.

Quando a porta estiver configurada como `Teclado USB`, registrar
`RETRO_ENVIRONMENT_SET_KEYBOARD_CALLBACK` e manter uma fila de transições. O callback não executa
guest diretamente; `retro_run()` drena a fila e chama `Session::set_key(avk, down)`. Isto evita
reentrância e mantém todo acesso ao emulador na thread normal do core. Usar o callback para
transições/texto, sem confundir `keycode` com caractere Unicode: uma tecla pode gerar múltiplos
caracteres, e eventos só de tecla ou só de caractere são válidos. Também fazer poll de
`RETRO_DEVICE_KEYBOARD` para estado sustentado, como teclas direcionais mantidas.

Mapeamento mínimo a testar/documentar:

| Tecla Libretro | AVK BREW |
|---|---|
| setas | `AVK_UP`, `AVK_DOWN`, `AVK_LEFT`, `AVK_RIGHT` |
| Enter | `AVK_SELECT` |
| Backspace/Escape | `AVK_CLR` |
| `0`–`9` | `AVK_0`–`AVK_9` |
| `*`/`#` | `AVK_STAR`/`AVK_POUND` |

RetroPad Select pode gerar `AVK_CLR` na rota de Z-Wheel como atalho, mas não substitui teclado
real. O teclado físico é global no Libretro; a seleção de `Teclado USB` na porta decide apenas o
que o guest enumera. Suportar duas portas de teclado distintas exigiria prova de que o BREW e a ABI
Libretro conseguem distingui-las.

### Mouse USB

Não anunciar `RETRO_DEVICE_MOUSE`, `RETRO_DEVICE_POINTER` ou mouse USB no MVP. O código atual não
implementa aparelho `Mouse`, enumeração HID de mouse, UIDs, botões ou eventos BREW correspondentes;
a busca no motor só encontra mouse da interface desktop. Também não há medida no corpus que prove
qual dispositivo USB/método BREW os títulos do Zeebo esperam para mouse.

Suporte futuro a mouse exige primeiro implementar e medir o dispositivo guest. Só então o core
poderá mapear `RETRO_DEVICE_MOUSE` (delta X/Y, botões e roda) ou `RETRO_DEVICE_POINTER` (posição
absoluta/touch) para esse protocolo e acrescentá-lo a `SET_CONTROLLER_INFO`.

## Rede do guest

A `Machine` atual nasce com rede ligada e pode usar `ureq` para atender `IWeb`. Esse padrão é
inadequado num core: abrir uma ROM não pode gerar conexão externa inesperada.

O core Libretro deve iniciar com rede **desligada**, sem ler `ZEEBX_SERVIDOR` nem outras variáveis
do processo. Quando houver uma opção funcional e documentada, ela pode permitir rede por decisão
explícita do usuário; host/porta de redirecionamento devem ser controlados pelo core, não por
ambiente global. Rede não deve ser chamada de thread auxiliar e falhas devem voltar ao guest como
erro BREW, sem pânico.

## Armazenamento, ROMs e savegames

### Layout Libretro

Em `retro_set_environment()`, antes de pedir quaisquer diretórios, negociar
`RETRO_ENVIRONMENT_GET_VFS_INTERFACE` com versão requerida 3. Essa versão traz criação e
listagem de diretórios, necessárias para cache/overlay. Guardar apenas interface emprestada pelo
frontend e encapsular operações atrás de `StorageFs`; nenhum caminho retornado pelo frontend pode
ser passado diretamente a `std::fs`/`fopen`.

`StorageFs` precisa cobrir open/read/write/seek, stat, mkdir, listagem, rename e remoção. Ele
precisa também representar `StoragePath` sem assumir `PathBuf`: um `saf://` pode não aceitar
`parent()`/`join()` nativos. Descoberta de `.mif`, recursos irmãos e extensão deve usar listagem e
junção de caminho fornecidas pelo adaptador. ZIP/7z devem receber leitor VFS; o ZIP precisa de
adaptador `Read + Seek`. Para os arquivos comuns, o cache/overlay usa operações VFS e rename
atômico também em caminhos SAF/Android; a compatibilidade completa desses caminhos continua
condicionada à solução SQLite descrita abaixo. Só materializar arquivo temporário local se o
frontend autorizar localização nativa e o arquivo for removido no unload.

O perfil usa **os dois** diretórios que o frontend fornece, com a mesma divisão que o PPSSPP faz
entre `flash0` (sistema) e o memory stick (saves):

```text
<system-dir>/zeebx/            # o que é da máquina ou descartável
├── aparelho/                  # fs:/ compartilhado entre títulos (a NAND do console)
└── cache/<conteúdo>/          # extração do pacote: descartável e o que enche disco

<save-dir>/zeebx/              # o que o jogador quer guardar
├── saves/<conteúdo>/          # overlay gravável do título
└── metadata/<conteúdo>.json   # índice de origem e identidade
```

**O nome `zeebx` é fixo e minúsculo.** Nunca pode sair do nome de exibição do core: o RetroArch já
cria as pastas dele com esse nome — `states/Zeebx`, `saves/Título.srm` — e num host que distingue
caixa (`saves/zeebx` e `saves/Zeebx` são pastas diferentes) o jogo passaria a ter dois perfis.

Sem `GET_SYSTEM_DIRECTORY` tudo cai no diretório de saves, que é o mínimo que a ABI garante.
Não alterar `HOME`, `XDG_CONFIG_HOME` ou diretório corrente do processo do frontend.

### Disco cheio

O cache guarda a extração de **todo** título já aberto; sem limite, um acervo de 62 jogos passa de
1,5 GB dentro do diretório do frontend e enche o disco de quem só queria jogar. Medido nesta
máquina: 66 zips somam 1,6 GB extraídos.

Regra: o core poda o cache a cada carga, mantendo a extração em uso e apagando as mais antigas até
caber em `CACHE_LIMIT_BYTES` (512 MB). A extração em uso nunca é removida, nem que sozinha estoure
o teto — apagar o jogo em execução seria pior que o disco cheio.

Disco cheio não pode travar o frontend. Duas regras de código sustentam isso:

1. o core **nunca** chama callback do frontend segurando um mutex próprio: os ponteiros são copiados
   antes da chamada. Um aviso do frontend ("disco cheio, quer salvar?") deixava de travar quando o
   `retro_run` passou a soltar o cadeado antes de vídeo e áudio;
2. falha de escrita no VFS é erro do guest, não pânico: o jogo recebe `IFAILED` e segue, e o motivo
   fica no relatório.

Se `GET_SAVE_DIRECTORY` não retornar diretório gravável, `retro_load_game()` deve falhar com
mensagem clara. Não usar a pasta da ROM como fallback e não aceitar execução que prometa save mas
o perca em diretório temporário.

### Três camadas

| Camada | Onde | Persistente | Compartilhada | Conteúdo |
|---|---|---:|---:|---|
| `cache/<conteúdo>/` | system | descartável | não | ROM extraída, `.mod`, `.mif`, assets |
| `saves/<conteúdo>/` | save | sim | não | arquivos relativos gravados pelo jogo |
| `aparelho/` | system | sim | sim | `fs:/`, Z-Wheel, dados globais |

Isto preserva o comportamento medido do console: Zeeboids pode gravar em
`fs:/zeeboiddata`, e Zeebo F.C. pode ler esses dados em outra sessão.

### VFS em camadas

Substituir raiz única atual por algo equivalente a:

```rust
pub struct Vfs {
    content_root: PathBuf, // somente leitura lógica
    save_root: PathBuf,    // overlay por jogo
    device_root: PathBuf,  // fs:/ compartilhado
    boundary: PathBuf,     // limite seguro de fs:/~/../
}
```

Regras:

- leitura relativa/`fs:/~/`: overlay primeiro, conteúdo base depois;
- criação/escrita relativa: sempre no overlay;
- `READWRITE`/`APPEND` de arquivo que existe só na base: copy-on-write para overlay;
- mapear `READWRITE`/`APPEND` ao modo VFS `UPDATE_EXISTING`, pois abrir write sem esse flag
  trunca arquivo existente;
- remoção de arquivo da base: criar tombstone no overlay, para que o arquivo não reapareça na
  próxima listagem/abertura;
- listagem: união overlay + base menos tombstones; overlay vence, comparação sem caixa e ordem
  determinística;
- `fs:/...`: `device_root`;
- `fs:/mod/...`: manter fallback atual entre aparelho e instalação;
- nunca alterar ROM, ZIP/7z original ou cache de conteúdo;
- preservar limites de `..`, caminhos absolutos e case-insensitivity do VFS atual.

A seleção do caminho deve ocorrer depois de conhecer o modo de abertura. Portanto,
`machine/file.rs` não pode escolher `vfs.resolve()` antes de saber se a operação é leitura,
criação, append ou read-write.

### Limite crítico: APIs de arquivo do motor e SQLite

A adaptação não é só trocar `loader/archive.rs`. Hoje o caminho de core usa `std::fs`/`PathBuf` em
`Session`, archive, `Vfs`, `machine/file.rs`, shell, mídia, fontes, widgets e SQL; `Machine` mantém
`std::fs::File` aberto. Além disso, `rusqlite` abre bancos por caminho nativo e pode criar journal
ou WAL. Um path `saf://` do frontend não é válido para essas APIs.

Antes de prometer Android/SAF, criar uma camada `StorageFs`/`GuestFile` usada por **toda** E/S do
motor, com backend nativo para desktop e backend `retro_vfs_interface` para frontend. Fazer
inventário de cada chamada `std::fs` que ainda entra no caminho de core; UI, testes e CLI podem
continuar nativos atrás da feature `desktop`.

SQLite exige decisão explícita:

1. implementar uma SQLite VFS sobre `retro_vfs_interface`; ou
2. materializar cópia transacional do banco num diretório nativo autorizado, abrir com SQLite e
   sincronizar de volta pelo VFS, incluindo journal/WAL e falhas de commit.

A opção 2 só é válida onde o frontend fornece local nativo autorizado; não serve para tornar
`SAF` magicamente compatível. Enquanto a opção não existir, o MVP deve declarar suporte a VFS
**nativo** (Linux x86_64/AArch64) e não prometer carga por SAF/Android. O critério de Android entra
depois de testes de SQLite, VFS, ZIP e save real nessa plataforma.

### Identidade

O fingerprint atual (`nome + tamanho + mtime`) não basta. Para ZIP/7z:

```text
content_hash = BLAKE3(bytes do arquivo compactado)
```

Para `.mod` direto, usar ao menos bytes do `.mod`, `.mif` associado e contexto de origem.

Diretório humano:

```text
<titulo-normalizado>-<classid>-<hash-curto>
```

Hash completo permanece na metadata. Atualização de ROM ganha save próprio por segurança; dados
globais do aparelho permanecem compartilhados.

## Extração ZIP e 7z

Não basta extrair `.mod` e `.mif`. Recursos, bancos, fontes e áudio podem ser abertos depois do
boot. Extrair toda a árvore validada.

ZIP já tem base em `loader/archive.rs`. Para 7z, usar decoder embutido/Rust ou biblioteca
portável. Não chamar binário externo `7z`: isto quebra AArch64, Android, sandbox e distribuição
portátil.

Antes de anunciar `7z`, validar com arquivos reais. Arquivos criptografados sem senha devem ser
recusados com mensagem clara.

Requisitos de segurança para ambos os formatos:

- recusar caminhos absolutos e `..`;
- recusar symlinks, hardlinks e nós especiais;
- limitar número de entradas;
- limitar tamanho de entrada e total descompactado;
- não extrair arquivo dentro de arquivo recursivamente;
- extrair para `<hash>.partial`;
- validar `.mod`/`.mif` e manifesto;
- renomear atomicamente para `<hash>` após sucesso;
- lock por hash para instâncias concorrentes.

### Cache ou `/tmp`

O cache fica em `<system-dir>/zeebx/cache/`, persistente entre sessões, para não descomprimir o
pacote em toda abertura — e **fora** do diretório de saves, que é o que o frontend sincroniza. Ele é
podado a 512 MB a cada carga (`CACHE_LIMIT_BYTES`), preservando a extração em uso.

Modo futuro opcional:

```text
zeebx_extract_mode = cache | temporary
```

No modo `temporary`, conteúdo pode ir para diretório temporário privado e ser removido no unload,
mas saves e `aparelho` continuam sempre sob `<save-dir>/zeebx/`. Nunca gravar save em `/tmp`.

### Concorrência e integridade

`aparelho/` representa um único console virtual e é compartilhado entre títulos. Duas instâncias
simultâneas podem corromper banco SQLite, configuração ou saves globais. A ABI VFS atual não oferece
lock exclusivo portável, portanto o core **não pode prometer** que bloqueará segunda instância em
SAF/sandbox só criando `<save-dir>/zeebx/.lock>`.

Implementar lock de perfil como melhoria best-effort no backend nativo, e tratar concorrência como
não suportada/documentada nos backends VFS sem lock atômico. O lock de cache por hash segue
separado: ele coordena apenas extração quando o backend permitir criação exclusiva. Não usar simples
"arquivo existe" como garantia de exclusão mútua.

Escritas de metadata, manifestos, tombstones e saves produzidos pelo core devem usar arquivo
provisório + rename atômico quando o backend VFS garantir essa operação. No unload, fechar e
sincronizar arquivos que o guest ainda mantém abertos. SQLite e outros writers precisam de política
própria de commit/journal; rename atômico de arquivos externos não protege seus bancos abertos.

## Environment callbacks úteis

| Callback | Uso |
|---|---|
| `SET_PIXEL_FORMAT` | RGB565 obrigatório no MVP |
| `GET_SAVE_DIRECTORY` | raiz de `zeebx/` |
| `GET_SYSTEM_DIRECTORY` | firmware/dados futuros |
| `GET_LOG_INTERFACE` | logs de load/VFS/erro |
| `SET_CONTROLLER_INFO` | dispositivos das portas |
| `SET_INPUT_DESCRIPTORS` | rótulos RetroPad |
| `SET_GEOMETRY` | somente quando geometria variar |
| `GET_CORE_OPTIONS_VERSION` | somente quando houver opções reais |
| `SET_CORE_OPTIONS_V2` | opções futuras |
| `GET_VARIABLE_UPDATE` | reler opções futuras |
| `GET_VFS_INTERFACE` | obrigatório antes de diretórios/load; toda E/S do core passa por ele |
| `GET_VFS_AUTHORIZED_LOCATIONS` | quando disponível, validar raízes VFS onde cache/arquivo pode existir |

Não registrar opção cosmética ou que não possa ser aplicada de forma segura numa sessão viva.

## Core Options: mecanismo e desenho para o Zeebx

O menu **Core Options** não é alimentado pelo `.info` do núcleo. O core registra as opções pelo
callback `retro_set_environment`, usando a ABI de `libretro.h`:

1. chamar `RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION` (`52`);
2. se a versão for pelo menos 2, registrar uma tabela estática com
   `RETRO_ENVIRONMENT_SET_CORE_OPTIONS_V2` (`67`), usando `retro_core_options_v2`, categorias e
   `retro_core_option_v2_definition`;
3. para frontends antigos, recuar para `RETRO_ENVIRONMENT_SET_VARIABLES` (`16`), com
   `retro_variable { key, "Nome; valor1|valor2" }`;
4. ler cada valor com `RETRO_ENVIRONMENT_GET_VARIABLE` (`15`), usando chaves prefixadas com
   `zeebx_`;
5. consultar `RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE` (`17`) no laço quando houver opção que possa
   mudar durante a sessão.

O frontend mantém uma cópia das strings e das tabelas. Portanto, descrições, valores e terminadores
precisam ter vida estática; não apontar para `String` local. O `key` deve ser estável e igual entre
versões para que o RetroArch preserve as escolhas. `retro_get_system_info` continua descrevendo só o
core e as extensões (`mod|zip|7z`); não há opção de usuário ali.

No código atual, `retro_set_environment` registra controladores e descritores de entrada, mas ainda
não registra opções. O próprio plano antigo deixava `GET_CORE_OPTIONS_VERSION`/`SET_CORE_OPTIONS_V2`
como futuro; a ABI e os structs necessários já estão no `frontends/libretro/include/libretro.h`.

### Primeira opção a implementar

A opção útil e verificável é uma política de síntese MIDI:

```text
key:     zeebx_midi_backend
category: audio
values:  Auto | Tabela de timbres | SoundFont
default: Auto
```

`Auto` preserva o comportamento atual: usa `.sf2` quando encontrado e recua para a tabela quando
não há banco ou o banco é inválido. `Tabela de timbres` evita a espera longa no RG40XX-H. `SoundFont`
recusa silenciosamente o recuo apenas depois de avisar que não há banco, ou mantém o recuo atual se a
compatibilidade for preferida. O texto da opção deve dizer que a troca exige recarregar o conteúdo.

A opção não pode ser apenas decorativa: hoje `Machine::new` chama `banco_do_aparelho` durante a
construção da sessão e guarda `banco_de_som`; `Session::start_*` e `Machine::new_with_storage` não
recebem uma política de áudio. A implementação precisa carregar a opção em `retro_load_game`, passá-la
pela criação da `Session`/`Machine` e também aplicá-la em `troca_para`, que recria a sessão quando a
Z-Wheel abre um jogo ou retorna a ela. Ler a opção no `retro_run` sem reconstruir a sessão deixaria o
menu dizendo uma coisa e o banco já carregado fazendo outra.

### Opções de desempenho, somente após medição

Não registrar ainda controles para `block_size` ou chorus/reverb antes de existir uma implementação
real. As candidatas são:

```text
zeebx_midi_effects: Ligados | Desligados
zeebx_midi_block:  64 | 1024
```

A medição local de 22/09/2026 mostrou 279 ms para a faixa de 47,5 s com bloco 64 e efeitos ligados,
e 149 ms com bloco 1024 e efeitos desligados. Isso dá cerca de 1,9x, mas não explica sozinho os mais
de dois minutos observados no H700; por isso a primeira opção deve ser o backend MIDI, e não expor
internais de desempenho como se fossem solução.

## Catálogo, identificação e capas

O acervo tem duas camadas de identidade, e as duas existem hoje:

| Identidade | Para que serve | Onde |
|---|---|---|
| BLAKE3 dos bytes do pacote | cache e overlay de saves: a mesma ROM abre no mesmo lugar, uma ROM trocada não herda save | `StoragePaths::content_id` |
| No-Intro (CRC32/MD5/SHA1 de um arquivo dentro do tape) | dizer **qual jogo é**, com o nome oficial, e achar capa | `ferramentas/catalogo.py` |

### O que o No-Intro de Zeebo hasheia

O DAT oficial é `Mobile - Zeebo` (libretro-database, `metadat/no-intro/`). Ele hasheia **um arquivo
dentro de `mod/<pasta>/`**, e o nome no DAT é o caminho interno sem as barras:

```text
mod274754sound.ggz                              -> mod/274754/sound.ggz
mod280386resources.pakz                         -> mod/280386/resources.pakz
modnfsresourcestracksworld_3401.viv             -> mod/nfs/resources/tracks/world_3401.viv
```

A pasta do módulo **nem sempre é numérica** (`mod/nfs/...`); comparar por prefixo erra o Need For
Speed. A comparação é pelo caminho inteiro sem separador.

### Medição contra o acervo do usuário

```text
pacotes: 62
verificados pelo No-Intro: 57 de 57 do DAT (0 divergentes)
fora do DAT: 5
```

Os cinco fora do DAT são os quatro ports da Data East e um homebrew:

- `Bad Dudes vs. DragonNinja`
- `Caveman Ninja`
- `Dark Seal`
- `Karnov's Revenge`
- `Kingdom Hearts V CAST (Zeebo Homebrew)`

### O que a ferramenta escreve

`ferramentas/catalogo.py --roms DIR --saida DIR [--icones]`:

- `Mobile - Zeebo.lpl` — playlist do RetroArch, `label` com o nome No-Intro, `crc32` do pacote e
  `db_name` do banco;
- `thumbnails/Mobile - Zeebo/{Named_Boxarts,Named_Snaps,Named_Titles,Named_Logos}/` — as quatro
  pastas que o RetroArch procura, com o nome exato de cada título;
- `catalogo.json` — hashes, verificação e o caminho do `.mod`/`.mif` de cada pacote.

### As capas oficiais vêm dentro da própria Z-Wheel

O pacote da Z-Wheel traz as capas da loja e o banco que as liga aos jogos:

```text
mod/274755/tt_game_info                     SQLite
  GAMEINFO(game_id, class_id, boxart_path, ...)   -> class_id é o ClassID do applet
  TITLETEXT(game_id, lang_id, titletext)          -> nome oficial por idioma
mod/274755/assets/games/<game_id>/boxartlg.jpg    170x220, a maior publicada
mod/274755/assets/games/<game_id>/boxart.bmp      160x227
```

A chave do cruzamento é o **ClassID do applet**, que o emulador já lê do `.mif` — não é nome de
arquivo nem de pasta. Medido contra o acervo:

```text
capas oficiais gravadas: 58 de 62
sem capa: Kingdom Hearts (homebrew, sem ClassID de applet no .mif)
          Z-Wheel, Zeebo App e Zenonia (não têm ficha na loja)
```

`--zwheel PACOTE` liga esse caminho; `--capas-ao-lado` copia a capa para o lado do `.zip`, que é
onde o frontend **standalone** procura (`library::cover` lê `<jogo>.png|jpg|bmp` ao lado do
arquivo). Com `--icones`, o ícone do `.mif` vai para `Named_Titles` como provisório — ícone de menu,
65×42 no máximo, não é capa.

O que ainda falta para "fullset publicado":

1. capas em resolução maior do que a da loja (170×220 é o que o pacote tem);
2. ~~RDB de Zeebo~~ — **feito**, e o "Scan Content" do RetroArch casa por hash;
3. envio das capas ao repositório de thumbnails do Libretro, com o nome do sistema igual ao do
   banco (`Mobile - Zeebo`), que é o diretório que o RetroArch procura — as capas **já estão** no
   formato e no nome certos, prontas para subir.

### RDB

**Resolvido.** O RDB de Zeebo não existia em lugar nenhum (135 RDBs instalados, nenhum de Zeebo) e
agora existe, gerado por `ferramentas/rdb.py` a partir do formato do próprio RetroArch
(`libretro-db/libretrodb.c` e `database_info.c`), conferido contra os RDBs oficiais:

```text
16 bytes     "RARCHDB\0" + uint64 **big-endian** com o offset dos metadados
registros    mapas msgpack em sequência, um por jogo
1 byte       0xC0, sentinela de fim
metadados    mapa msgpack { "count": N }
```

Campos de cada registro, iguais aos dos RDBs oficiais: `name`, `description`, `rom_name`, `size`,
`crc` (binário de 4 bytes, big-endian), `md5`, `sha1`.

Cada jogo entra **duas vezes**: com o CRC do `.zip` e com o CRC do arquivo que o No-Intro hasheia
dentro dele. O scanner consulta `crc:or(b"<arquivo de dentro>", b"<pacote>")`, então o acervo casa
pelo pacote e também pelo ROM interno — este último sobrevive a recompactar o zip.

Medido, com o core instalado:

```text
[Scanner]: Add "Double Dragon (Brazil) (Es,Pt)" to "Mobile - Zeebo.lpl"
[Scanner]: Add "Zeebo Sports Peteca (Brazil) (Es,Pt)" to "Mobile - Zeebo.lpl"
[Scanner]: Add "Caveman Ninja (Brazil) (Es,Pt)" to "Mobile - Zeebo.lpl"
[Scanner]: Add "Zenonia (Brazil) (Es,Pt)" to "Mobile - Zeebo.lpl"
```

As duas últimas linhas não estão no No-Intro nem têm ficha na loja: elas casam pelo registro de
**CRC do `.zip`**, que é gerado para todos os 62 pacotes. Ou seja, o banco nomeia o acervo inteiro,
não só a parte verificada pelo DAT.

Duas condições para o scan achar o banco, e as duas são fáceis de esquecer:

1. o banco precisa ser o `<nome do database>.rdb` dentro de `content_database_path` (na config do
   usuário: `~/.config/retroarch/database/rdb`), com o nome igual ao campo `database` do `.info`;
2. o scanner padrão exige que o conteúdo já case com um **core instalado**
   (`scan_without_core_match = "false"`). Sem o core na pasta de cores, ele nem entra na fase de
   banco e marca `??` em tudo. Com o core instalado funciona; se ainda assim não casar, ligue
   `scan_without_core_match`.

### Destinos explícitos, e o `.info` junto do core

Duas lições desta etapa, as duas medidas na máquina:

1. **o core e o `.info` andam juntos.** Um `.info` antigo, sem a linha `database`, instalado ao lado
   de um core novo, quebra o scan pelo menu: o RetroArch não associa banco nenhum àquele core, e
   todo conteúdo sai como `??`. Foi exatamente o que aconteceu aqui — o `.info` tinha sido copiado
   antes de a linha existir;
2. **nenhum artefato pode cair na pasta do frontend por acidente.** `--saida` era usado para
   playlist, catálogo e cache do DAT, e apontá-lo para a configuração do RetroArch largou
   `Mobile - Zeebo.lpl`, `catalogo.json` e `Mobile - Zeebo.dat` na raiz dela. Agora playlist,
   catálogo e DAT têm destino próprio (`--playlists`, `--catalogo` e o DAT ao lado do catálogo).

### Playlist

O RetroArch 1.20 grava playlist como **um documento JSON** com cabeçalho e `items` — não o formato
antigo de um JSON por linha. O `crc32` leva o sufixo `|crc` e o `db_name` leva `.lpl`. A ferramenta
copia o formato de quem lê, em vez de inventar.

## Desfecho de um jogo

No console, sair de um jogo devolve o controle à Z-Wheel, que é apenas outro applet instalado. No
Libretro isso tem duas formas possíveis, e elas não são equivalentes:

| Caminho | Como funciona | Estado |
|---|---|---|
| Frontend encerra o conteúdo | o core chama `RETRO_ENVIRONMENT_SHUTDOWN` (7) | **implementado** |
| Core carrega a Z-Wheel | o próprio core inicia outra `Session` com o `.mod` da Z-Wheel e segue apresentando | **implementado** |

O motor já sabe que o shell pediu outro applet: `Machine::pending_launch`, exposto por
`Session::take_launch_request()`, é o que a UI desktop usa para voltar à Z-Wheel. O que **não**
existe na ABI é um "carregue este outro conteúdo" do core para o frontend — nenhum comando faz o
frontend trocar de jogo a pedido do core. Logo, a volta à Z-Wheel só pode acontecer se o core a
carregar internamente.

### Ligação feita no core

O motor já tinha as peças; faltava o core usá-las.

| Peça | Como ficou |
|---|---|
| Botões | os quatro de face seguem o aparelho: **1 embaixo** (`B` do RetroPad, `X` do PlayStation, `B` do SNES) e a numeração sobe no sentido horário — 2 à esquerda (`Y`), 3 em cima (`X`), 4 à direita (`A`). Os rótulos de mapeamento apontam para o mesmo `id` que o core lê |
| Vídeo | **1x e proporção nativa, sempre**: `define_resolucao_interna(1)` e `define_proporcao(None)` na carga e na troca de sessão. Quadro fora de 640×480 é avisado uma vez no log, em vez de aparecer como imagem torta sem explicação |
| Teclado | `SET_KEYBOARD_CALLBACK` recebe as teclas e **enfileira**; `retro_run` entrega os `AVK`. Mapeados: setas, Enter, Backspace/Esc, dígitos, `*` e `#`. O Select do RetroPad vale como `AVK_CLR` |
| Biblioteca | `library::scan` na pasta do conteúdo, deduplicada por ClassID, alimenta `set_installed_applets` — sem isso a Z-Wheel abre vazia |
| Troca de sessão | o pedido de lançamento encerra a sessão atual e inicia a nova com o mesmo `StoragePaths`; o frontend nem percebe |
| Volta à Z-Wheel | ao terminar o jogo, se a sessão era a Z-Wheel ou foi aberta por ela, o core volta para ela em vez de pedir `SHUTDOWN` |
| Fonte do sistema | `fonte_do_sistema_em(cache, aparelho)` copia o `tectoy.ttf` do pacote para a raiz do aparelho do frontend. Se o acervo só tiver a Z-Wheel extraída, `instala_fonte_do_pacote` a pega de ao lado do módulo |

Medido, headless, com a Z-Wheel como conteúdo:

```text
Zeebx: 63 jogo(s) instalados a partir de /media/.../zeebo/ROMs
Zeebx: fonte do sistema em .../system/zeebx/aparelho/shared/fonts/tectoy.ttf
tela: 640x480, 398 cores distintas
```

Os 63 são 62 títulos mais a própria Z-Wheel; antes da deduplicação por ClassID eram 124, porque a
pasta de ROMs tem os `.zip` **e** uma cópia já extraída.

Comportamento atual:

1. o desfecho é relatado **uma vez**, com o relógio virtual (`parou em N ms virtuais`), seguido das
   últimas linhas do log do próprio jogo e do uso de heap;
2. se o shell pediu um applet e ele está na pasta de jogos, a sessão troca; se não está, o ClassID
   vai para o log dizendo que não foi encontrado;
3. a tela final fica à mostra **2 segundos**; então o core volta à Z-Wheel, se ela estiver
   disponível, ou pede `SHUTDOWN` para o frontend voltar ao menu dele;
4. o pedido de descarga é feito **fora** do mutex do estado, como todas as chamadas ao frontend.

O caminho do core-carrega-a-Z-Wheel exige resolver o ClassID para um pacote instalado — o core ainda
não tem varredura de biblioteca — e decidir a política de "próximo conteúdo" com o frontend. Fica
como feature, com o motor já pronto para iniciar uma sessão nova.

### Instalação e CI

- `ferramentas/instala_core.py` copia o `.so` **e** o `.info` **juntos**. Os dois andam em par: um
  `.info` velho ao lado de um core novo quebra o scan pela interface, sem dizer nada — já aconteceu
  aqui. O script avisa quando o `.info` de destino era diferente do que vai entrar;
- `.github/workflows/libretro.yml` compila os dois alvos — **x86_64** e **AArch64 em runner ARM64
  nativo**, porque cross-build de `unicorn`/`dynarmic` exigiria toolchain e sysroot que ninguém quer
  manter só para empacotar — e confere três coisas, nenhuma delas "compilou":
  1. os 25 símbolos `retro_*` estão exportados (`nm -D`);
  2. o `.so` não puxa `libX11`, Wayland, EGL, ALSA, udev nem GTK (`ldd`);
  3. o pacote publicado leva o `.info` com o campo `database`.

### Avisos ao jogador e leitura de botões

- `SET_MESSAGE` mostra avisos na tela do frontend ("sem `tectoy.ttf`", "o shell pediu uma classe que
  não está na pasta"), além de registrá-los no log — nem todo frontend exibe mensagem, e aviso que
  ninguém vê não serve;
- os botões são lidos por **máscara de bits** quando o frontend anuncia suporte
  (`GET_INPUT_BITMASKS`), o que reduz doze consultas por quadro a uma. **O RetroArch 1.20 devolve
  falso**, então na prática ele usa consulta individual; o caminho da máscara fica para frontends
  que o anunciem. O log diz qual dos dois está em uso, em vez de deixar isso invisível.

## O que falta para os ports da Data East: a interface `IFont`

Investigando o quadro branco do Double Dragon, a causa **de fundo** apareceu: ele pede três classes
que respondemos como desconhecidas.

```text
classes que o jogo pediu e não temos:
  0x0102f679   AEECLSID_FONT_STANDARD11
  0x01030852   AEECLSID_FONT_STANDARD15
  0x0102f681   AEECLSID_FONT_STANDARD36
```

São as **fontes de bitmap padrão do BREW**, e o `AEEFontsStandard.bid` do SDK confirma o que cada uma
é (ascent/descent documentados) e qual interface implementam:

```c
#define AEECLSID_FONT_STANDARD11  0x0102f679   /* IFont */
#define AEECLSID_FONT_STANDARD15  0x01030852   /* IFont */
#define AEECLSID_FONT_STANDARD36  0x0102f681   /* IFont */
```

**`IFont`, não `ITypeface`** — é essa a diferença que importa, e é por isso que o mapeamento não
podia ser só "apontar para a nossa fonte TrueType". O `AEEFont.h` lista os seis métodos, e um
emulador de referência já no disco (`projects/zeebo-emulator/.../zeemu/brew/BrewFont.cpp`) dá a
ordem da vtable e a tabela de métricas de todas as classes:

```c
AddRef(0)  Release(1)  QueryInterface(2)  DrawText(3)  GetInfo(4)  MeasureText(5)
```

A família inteira são 30 classes: `FONTSYS*` (6), `FONT_STANDARD*` (11), `FONT_BASIC*` (10),
`FONT_FIXED4X6`, `AEECLSID_FONT` e o `BITFONTFOUNDRY`. As assinaturas saem dos próprios exemplos do
SDK:

```c
IFont_GetInfo(pFont, &info, sizeof(info))
IFont_MeasureText(pFont, pszBuf, WSTRLEN(pszBuf), IFONT_MAXWIDTH, &nChars, &extent.width)
```

O que fazer, na ordem que o projeto adota — **medir antes de inventar**:

1. usar a sonda já existente (`--sonda=0x0102f679`) numa ROM que peça a classe, para registrar por
   qual slot o jogo chama e com que argumentos. O `DrawText` é o único com layout ainda não medido;
2. implementar `Interface::Font` com os seis slots, `GetInfo` escrevendo as quatro métricas e
   `MeasureText` devolvendo largura e altura da tabela;
3. mapear as 30 classes para essa interface, e o `DrawText` desenhando no destino corrente — o mesmo
   caminho que o `IDISPLAY_DrawText` já usa.

Sem isso, todo título que desenha com as fontes do sistema cai no mesmo lugar: o Double Dragon, o
Resident Evil 4 (`0x0102f681`) e os ports da Data East.

## Save states

Save states exigem snapshot pointer-free e versionado de:

- CPU Dynarmic;
- memória guest;
- heap e objetos BREW;
- callbacks, sinais, threads e timers;
- VFS relevante;
- áudio;
- rasterizador e recursos gráficos;
- extensões e estado de applet.

Quando implementado, `retro_serialize_size()` não pode crescer entre `retro_load_game()` e
`retro_unload_game()`. Validar:

```text
salva → executa N frames → restaura → executa N frames
```

Comparar relógio, framebuffer, memória e áudio conforme aplicável.

Save state não deve fingir ser rollback de arquivos persistentes: `saves/` e `aparelho/` ficam fora
do snapshot e mantêm a última escrita confirmada no host. Esta regra deve aparecer na documentação
do recurso quando ele existir. Qualquer estado que mantenha ponteiro, descritor de arquivo ou
recurso de host precisa ser reconstruído a partir de representação serializada, nunca copiado cru.

## Distribuição e builds

Alvos iniciais:

```text
x86_64-unknown-linux-gnu
aarch64-unknown-linux-gnu
```

Artefato distribuído:

```text
zeebx_libretro.so
```

O host atual só possui o target Rust x86_64. AArch64 requer target Rust, linker, C/C++20,
CMake, Ninja e sysroot coerentes. `unicorn-engine` compila C/QEMU; Dynarmic compila C++20/CMake;
Linux também liga `libatomic` conforme `build.rs` atual.

O build local de `cargo check --locked` chegou à build de Dynarmic, mas falhou por falta de espaço
em disco enquanto CMake/Ninja construía centenas de alvos — não por erro de fonte. CI e cross-build
precisam de espaço de disco adequado e cache de dependências.

Recomendação:

1. validar x86_64 localmente;
2. usar runner AArch64 nativo primeiro;
3. adicionar cross-build após toolchain/sysroot estar comprovado;
4. testar carregamento real em RetroArch, não apenas compilação/dlopen.

CI deve verificar o pacote separado:

```text
cargo build --locked --release -p zeebx-libretro --target x86_64-unknown-linux-gnu
cargo build --locked --release -p zeebx-libretro --target aarch64-unknown-linux-gnu
readelf -h
nm -D / checagem de símbolos retro_*
ldd
smoke test RetroArch
```

A release coleta somente `target/<triple>/release/libzeebx_libretro.so` e o
`frontends/libretro/zeebx_libretro.info`; não deve incluir binário desktop, settings da UI ou
artefatos de outros membros do workspace.

## Arquivo `.info`

Distribuir `zeebx_libretro.info` junto do `.so`, por exemplo:

```ini
display_name = "Zeebx"
authors = "ZeebxTeam"
supported_extensions = "mod|zip"
categories = "Emulator"
systemname = "Zeebo"
manufacturer = "TecToy"
licenses = "GPLv2"
firmware_count = 0
supports_no_game = "false"
```

Adicionar `7z` somente depois de suporte real.

## Validação executada

### Ferramenta

RetroArch 1.20.0 com perfil isolado em `/tmp/zeebx-ra` (nunca o perfil do usuário), drivers
`video = null`, `audio = null`, `input = null`, `--max-frames=N` e screenshot final:

```bash
XDG_CONFIG_HOME=/tmp/zeebx-ra retroarch -c /tmp/zeebx-ra/ra.cfg \
  -L /tmp/zeebx-ra/cores/zeebx_libretro.so --max-frames=900 \
  --max-frames-ss --max-frames-ss-path=saida.png "'ROM.zip'"
```

### O que o frontend confirmou

| Etapa | Resultado |
|---|---|
| Símbolos exportados | 25 funções `retro_*` |
| `ldd` do `.so` | só `libc`, `libm`, `libstdc++`, `libgcc` — sem X11, Wayland, EGL, ALSA, udev, GTK |
| `cargo tree -p zeebx-libretro` | sem eframe, cpal, gilrs, glutin, minifb, rfd, Discord |
| `retro_api_version` | 1 |
| `retro_get_system_av_info` | 640×480, aspecto 1,333, 60 Hz, 44 100 Hz |
| `SET_CONTROLLER_INFO` / `SET_INPUT_DESCRIPTORS` | aceitos |
| `SAVE_DIRECTORY` / `SYSTEM_DIRECTORY` | usados; perfil criado em `<save>/Zeebx/zeebx/` |
| `SET_PIXEL_FORMAT` | RGB565 aceito |
| Extração | `cache/<título>-<hash BLAKE3>/mod/<id>/<nome>.mod` |

### Por jogo

| Título | Quadros | Tempo relatado pelo frontend | Quadro entregue |
|---|---:|---:|---|
| Zeebo Sports Peteca | 900 | 14 s | 2 734 cores distintas |
| Crash Bandicoot Nitro Kart 3D | 900 | 14 s | 5 959 cores distintas |
| Double Dragon | 3 600 | 59 s | branco uniforme |

**O quadro branco do Double Dragon: resolvido, e eram três defeitos empilhados.**

1. `fonte_do_sistema()` procurava no cache **do desktop** em vez do cache que o frontend forneceu;
2. `fonte_do_console()` — a busca na hora de desenhar — também usava o caminho do desktop, então a
   fonte instalada em `<raiz do aparelho>/shared/fonts/tectoy.ttf` nunca era encontrada;
3. **e a ordem estava errada**: a fonte era instalada **depois** de a sessão existir. O motor lê a
   fonte quando a máquina é construída, então a sessão inteira ficava sem fonte nenhuma.

O relatório do próprio jogo já dizia o que faltava:

```text
texto que não soubemos desenhar:
  Application is finished
  Memory is insufficient.
  Please delete
  by pushing the button.
  some files.
```

A tela do Double Dragon é **branco com essa mensagem em preto**. Sem fonte, saía branco puro — que
parecia "não desenha nada" e era "desenha texto invisível". Medido, antes e depois:

```text
antes:  1 cor   (branco puro)
depois: 2 cores (branco + preto) — o texto aparece
```

O que a mensagem revela é outra coisa, e essa continua: o Double Dragon pede três classes que não
temos (`0x0102f679`, `0x0102f681`, `0x01030852`) e não acha `./udata/ddz.sav`, e por isso mostra
"Memory is insufficient". O emulador agora **mostra** o problema em vez de escondê-lo.

Peteca e Crash provam vídeo real pelo caminho Libretro. Double Dragon carrega, roda 59 segundos
virtuais sem erro e não quebra, mas entrega quadro branco.

**Pendência de investigação, não defeito da ABI.** A tabela de `docs/implementacao/11-compatibilidade.md`
foi medida com o caminho `zeebx run` (**Unicorn**, `src/main.rs::run_frames`), enquanto o core usa
`Session` (**Dynarmic**). Antes de acusar o core é preciso rodar a mesma ROM nos dois caminhos e
comparar; o próprio documento lista Double Dragon com "ponteiro recusado, texto sem fonte".

### O que os testes no Wayland acharam

Rodando o Crash Nitro Kart no RetroArch do usuário (Wayland + Vulkan, driver de vídeo real), o log
do core mostrou o defeito mais caro desta etapa:

```text
Zeebx: parou em 19640 ms virtuais: o jogo terminou
```

O jogo rodava 19,6 s e a sessão se encerrava sozinha. Causa: `Machine::is_idle()` decidia que a
máquina estava ociosa olhando timers, callbacks, threads e blits — mas **não** olhando
`pending_signals` nem o fato de o guest ter registrado interesse em entrada. O intro do Crash acaba
exatamente quando ele passa a esperar o jogador: sem timer armado e com o callback de entrada
registrado, o emulador concluía "acabou". Depois da correção o mesmo jogo passa de 27,7 s, e um
título que fica esperando botão continua rodando, como no console.

Segundo achado, ainda no Wayland: com o disco da máquina cheio, o RetroArch abriu o aviso de
gravação e o emulador travou. O core chamava `video_refresh` e `audio_sample_batch` segurando o
mutex do estado, e também chamava o callback de ambiente segurando o mutex dos ponteiros do
frontend. Agora os ponteiros são copiados e os cadeados soltos antes de qualquer chamada ao
frontend, e os buffers de quadro e áudio saem do estado antes de vídeo/áudio e voltam depois.

### Higiene dos testes

A suíte deixava lixo: os testes de SQL e de saves criavam `/tmp/zeebx-sql-*` e `/tmp/zeebx-saves-*`
e limpavam **no começo** — para o caso de a rodada anterior ter falhado — e nunca no fim. Uma sessão
com várias rodadas acumulou dez diretórios. Agora existe `src/scratch.rs::TempDir`, que apaga a
pasta quando o teste sai de escopo, passe ele ou não. Verificado: depois da suíte inteira, `/tmp`
não tem nenhum `zeebx-*`.

### Testes automatizados

```text
cargo test --lib --locked
→ 442 passaram, 0 falharam, 9 ignorados (com as features de desktop)

cargo test --lib --locked --no-default-features
→ 384 passaram, 0 falharam, 5 ignorados (só o motor, que é o que o core usa)

cargo test --lib --locked brew::vfs      → 14 passaram
cargo test --lib --locked loader::archive → 6 passaram
cargo test --lib --locked storage::      → 3 passaram
```

### Como repetir

```bash
CARGO_PROFILE_DEV_DEBUG=0 CARGO_BUILD_JOBS=1 cargo build -p zeebx-libretro
cp target/debug/libzeebx_libretro.so  <pasta de cores>/zeebx_libretro.so
cp frontends/libretro/zeebx_libretro.info <pasta de cores>/
```

O `DEBUG=0` não é detalhe: com debuginfo a árvore passa de 5 GiB e, nesta máquina, o disco acabou
no meio da compilação mais de uma vez.

## Referências e padrões verificados

Esta seção registra fontes externas usadas para decisões de arquitetura. Não copiar código C/C++
sem revisar licença, segurança e adaptação a Rust.

### Documentação/API oficial

- [Libretro Input API](https://docs.libretro.com/development/input-api/) — semântica de RetroPad,
  teclado, mouse e pointer;
- [libretro.h canônico](https://github.com/libretro/libretro-common/blob/master/include/libretro.h)
  — ABI, `RETRO_DEVICE_SUBCLASS`, sensor, VFS, diretórios e lifecycle;
- [libretro-common archive 7z](https://github.com/libretro/libretro-common/blob/master/file/archive_file_7z.c)
  — prova de que a infraestrutura oficial trata 7z; não elimina os limites de segurança do Zeebx.

### Cores oficiais examinados

| Core | Referência | Prática aproveitada |
|---|---|---|
| Flycast | [`core/libretro/libretro.cpp`](https://github.com/libretro/flycast/blob/master/core/libretro/libretro.cpp) | subclasses de joystick, `SET_CONTROLLER_INFO`, callback de teclado, VFS solicitado cedo, refresh de dispositivos/descriptors depois de troca de porta |
| Dolphin | [`DolphinLibretro/Input.cpp`](https://github.com/libretro/dolphin/blob/master/Source/Core/DolphinLibretro/Input.cpp) | sensor por porta: habilitar, aceitar somente retorno de sucesso, transformar orientação, usar por frame e desabilitar no shutdown |
| Dolphin | [`Main.cpp`](https://github.com/libretro/dolphin/blob/master/Source/Core/DolphinLibretro/Main.cpp), [`Boot.cpp`](https://github.com/libretro/dolphin/blob/master/Source/Core/DolphinLibretro/Boot.cpp), [`VFile.cpp`](https://github.com/libretro/dolphin/blob/master/Source/Core/DolphinLibretro/Common/VFile.cpp) | `need_fullpath`+`block_extract`, layout de usuário, VFS negociado e modos de abertura/append/truncate explícitos |
| ScummVM | [`libretro-core.cpp`](https://github.com/libretro/scummvm/blob/master/backends/platform/libretro/src/libretro-core.cpp) | aceitar somente tipos/portas de input suportados, teclado por callback, VFS/authorized locations e save states declaradamente ausentes |
| DOSBox Pure | [`dosbox_pure_libretro.cpp`](https://github.com/schellingb/dosbox-pure/blob/main/dosbox_pure_libretro.cpp) | decode explícito de subclasses, teclado/mouse/pointer separados, save no diretório do frontend e serialização de tamanho fixo/validada |
| PPSSPP | [`libretro.cpp`](https://github.com/libretro/ppsspp/blob/master/libretro/libretro.cpp) | separação de system/save directory; o fallback dele para diretório do conteúdo **não** será copiado porque Zeebx não deve escrever ao lado da ROM |

Consequências incorporadas neste plano:

- VFS deixa de ser fase posterior: é requisito da arquitetura desde a primeira carga; suporte
  pleno a paths não-nativos depende também de adaptar SQLite e todas as E/S do motor;
- subtipo não é tipo de poll; Zeebx faz poll do dispositivo-base;
- sensor só vira fonte Boomerang se `set_sensor_state()` retornar sucesso;
- teclado usa callback e poll, com fila para não reentrar no guest;
- mouse/pointer só entram depois de haver aparelho BREW medido;
- 7z anunciado somente após parser seguro e testes de corpus.

## Matriz de build

Seis alvos por artefato — o core e o standalone —, três sistemas × duas arquiteturas, **cada um
no seu runner nativo**:

| | x86_64 | AArch64 |
|---|---|---|
| Linux | `ubuntu-24.04` | `ubuntu-24.04-arm` |
| Windows | `windows-latest` | `windows-11-arm` (experimental) |
| macOS | `macos-15-intel` | `macos-latest` (Apple Silicon) |

Nada aqui é cross-compilação, e é de propósito: o `unicorn` compila o QEMU em C e o `dynarmic`
compila C++20, exatamente a combinação em que cross-build exige toolchain C++ e sysroot. Existe
runner ARM64 nativo para os três sistemas, e ele sai mais barato que manter cross.

O core é publicado como `<alvo>/zeebx_libretro.{so,dll,dylib}` junto do `.info`, e o standalone
como o binário do alvo. Cada artefato passa por duas provas antes de subir:

1. **A biblioteca carrega e a ABI responde** (`ferramentas/verifica_core.py`): abre a biblioteca
   com o `dlopen`/`LoadLibrary` por trás do `ctypes` — como o frontend abre —, confere os 25
   símbolos, exige `retro_api_version` igual a 1 e exige que `retro_get_system_info` preencha
   `Zeebx`, `mod|zip` e os dois sinalizadores. Ler símbolos com `nm` não apanharia uma biblioteca
   que existe e **não carrega** por dependência que faltou no link; carregar, sim.
2. **Sem dependência de interface** (Linux): `ldd` recusa `libX11`, Wayland, EGL, ALSA, udev e GTK.
   Vídeo, áudio e entrada são do frontend.

### Windows ARM64 é experimental, e a causa é medida

O alvo existe na matriz, roda, e **não** derruba a rodada. Ele não fica verde por causa de uma
limitação de terceiros:

```text
FAILED: CMakeFiles/unicorn-common.dir/qemu/util/setjmp-wrapper-win32.asm.obj
  ml  -I...unicorn-engine-sys-2.1.5\msvc -I...
  CreateProcess failed: The system cannot find the file specified.
```

O QEMU monta esse `.asm` com o `ml` do MSVC, que **só existe para x86 e x64** — não há montador
MASM para ARM64. O `unicorn-engine` 2.1.5 é a última versão publicada, e o QEMU não tem o Windows
ARM64 como host. Quando isso mudar — versão nova do unicorn, ou o `unicorn` virar opcional nesta
plataforma, com o `dynarmic` sozinho —, basta tirar a marca de experimental do alvo.

## Estado dos itens, com a prova de cada um

| Item | Estado | Prova |
|---|---|---|
| Varredura das 62 ROMs | feito | 56 rodam (linha de base do doc: 50), 0 estados piorados — reconferido depois de todas as mudanças desta sessão. Os 6 fora mudaram de nome: o **Zuma saiu** (passou a rodar) e o **Prey Evil entrou** na contagem, porque antes ele aparecia como "roda" com a tela preta |
| CI dos seis alvos do standalone | **feito** | `ci 77d5b02`: linux x86_64 e AArch64, macOS Intel e Apple Silicon, Windows x86_64 e ARM64 — **os seis verdes**, e o core também |
| `IFont` e o layout do `DrawText` | feito | métricas transcritas do `AEEFontsStandard.BID`, com teste que cobra as onze classes |
| Áudio e desempenho | feito | varredura mede pico/rms/contínuo/salto por jogo; Rolima 79% → 284%, 51 jogos mais rápidos; e **pelo caminho do core**: o Peggle entrega 229.080 amostras estéreo em 120 quadros |

### A varredura pegou uma "regressão" que era uma verdade

Depois das seis extensões e do joystick, uma varredura de conferência mostrou **um jogo a mais fora
de "roda"**: 7 em vez de 6. O jogo era o **Prey Evil**, e a leitura fácil seria "as extensões
quebraram alguma coisa" — não quebraram.

Ele estava **nos dois casos igualmente quebrado**. Antes o relatório dizia `roda`, porque o jogo
executava, respondia API e apresentava quadro: o que não fazia era desenhar, e a varredura de então
não olhava a tela. Agora ele diz `quebrou no laço de quadros`, que é o que sempre foi — e as seis
extensões são o que o levou até o ponto onde a quebra acontece, em vez de parar no primeiro portão
fechado.

Os outros seis jogos fora de "roda" são exatamente os mesmos de antes, sem mudança de estado em
nenhum. A conferência valeu por dois motivos: confirmou que as mudanças de hoje não mexeram em jogo
nenhum, e mostrou que **a métrica de cores trocou um rótulo falso por um verdadeiro** — que é o
serviço dela.

### O Zuma: o decodificador só sabia PNG

A classe `AEECLSID_JPEGDecoderBREW` entrou na fábrica (uma linha), e o jogo **avançou um portão** —
a lista de classes faltantes ficou vazia e a falha mudou de `0x1a6b8` para `0x1a72c`. O rastreio,
ligado logo depois, entregou a causa da segunda:

```text
IImageDecoder::QueryInterface  → 0    (SUCCESS)
IForceFeed::Reset / Write      → 0    (o jogo alimentou o decodificador, com 16 KB de dados)
IImageDecoder::GetBitmap       → 0x1  ← FALHOU, e o jogo segue com o bitmap nulo
```

E o motivo está em `machine/image.rs`: `decoded_bitmap` chama **`decode_png`** e, quando os bytes
não são PNG, registra a hipótese "um decodificador recebeu dados que não são um PNG" e devolve 0.
O jogo pediu o decodificador de **JPEG** e alimentou um JPEG.

**A correção é curta e o despachante já existe**: `video::icon::decode(&bytes)` escolhe o formato
**pela assinatura** — PNG, BMP ou JPEG — e é o mesmo que o emulador usa para os ícones dos módulos.
Falta ligar esse caminho ao decodificador do guest, convertendo a imagem para a estrutura que o
`decoded_bitmap` publica (o `DecodedImage`, com o DIB em 24 ou 32 bits que o `publica_dib_do_png`
já sabe montar). É a próxima peça, e é a mesma receita que fechou sete portões hoje: medir, estreitar
a lista, corrigir o que falta — e a varredura diz se andou.

### Os 18 estalos da Peteca não eram do mixer

O item 4 pede uma revisão de ruído, e a medição do áudio acusou 18 saltos acima de meio curso em
seis segundos de Peteca, o maior deles em 0,947. Havia um suspeito claro no código: a `Voice`
**sumia de uma vez** quando o som acabava — o nível podia estar em 0,947 e a amostra seguinte era
zero. O caminho do `Stream`, no mesmo arquivo, já evitava isso ("segura o último valor em vez de
estalar para o zero"); a voz não.

Implementada a descida de [`DESCIDA_FRAMES`] quadros (1,5 ms), com teste que cobra a rampa monótona.
E a medição, depois:

```text
antes:  maior salto 0,947 · 18 acima de 0,50
depois: maior salto 0,947 · 18 acima de 0,50     ← idêntico
```

**Os saltos são os ataques dos efeitos**, que começam longe do zero no próprio dado do jogo — e um
ataque percussivo legítimo é indistinguível de um estalo pela métrica, que só vê a saída misturada.
A conclusão honesta do item 4 é dupla: **o mixer não era a causa** destes 18, e o corte de voz que
era uma causa possível deixou de existir.

### A tela que a varredura não olhava

A varredura contava **escritas** na tela, e um jogo que pinta 307.200 pixels de preto conta 307.200
escritas: registrava "roda" com a tela apagada. Agora o relatório conta **cores distintas e a
dominante**, e a linha entra na comparação com a linha de base — uma regressão que apaga a tela sem
quebrar a execução passa a aparecer no commit que a causou.

**A varredura inteira fecha contra a linha de base commitada, e continua fechando.** Depois de
todas as mudanças do dia — as seis extensões, o joystick, a classe do decodificador de JPEG, o
despacho por assinatura e a descida da voz no mixer —, rodadas as 62 ROMs de novo:

```text
62 relatórios · 0 diferenças · 0 linhas de base gravadas
```

Nenhum jogo mudou de resumo sem que a mudança fosse intencional e registrada, e a árvore saiu limpa.

**A varredura inteira fecha contra a linha de base commitada.** Rodadas as 62 ROMs depois de todas as
mudanças do dia — as seis extensões, o joystick, a classe do decodificador de JPEG e o despacho por
assinatura —, o resultado foi **zero diferença** e **zero linha de base gravada**: nenhum jogo mudou
de resumo sem que a mudança fosse intencional e registrada (o Zuma, que passou a rodar), e nenhum
outro se moveu. É o que o item 1 do plano pede, e agora é automático: a pasta `docs/varredura/` está
no repositório com as 62 linhas.

**A contagem entra na linha de base, então ela precisava ser estável** — uma métrica que varia
entre execuções acusaria regressão falsa a cada varredura, e um teste que acusa sempre não é lido
nunca. Medido com o Pac-Mania, três execuções seguidas:

```text
execução 1: tela: 695 cor(es), dominante 0x0004
execução 2: tela: 695 cor(es), dominante 0x0004
execução 3: tela: 695 cor(es), dominante 0x0004
```

É o esperado — o quadro final é função do tempo **virtual**, que a varredura controla — e é o que
permite à linha de base cobrar a linha inteira, e não só o estado.

Na primeira varredura com a métrica, duas telas pretas entre os que "rodam", e dar mais tempo
virtual separou os dois casos possíveis:

| Jogo | 6 s | 20 s | Leitura |
|---|---:|---:|---|
| Zeebo F.C. Foot Camp | 1 cor | **2410 cores** | estava carregando: seis segundos é pouco |
| Zeebo F.C. Super League | 6 cores | **4133 cores** | idem |
| **Prey Evil** | 1 cor | **1 cor** | **não desenha nada, e roda a 3287%** |

Os dois primeiros são calibração: **seis segundos não bastam para quem carrega antes de desenhar**, e
o número de cores é o que denuncia isso sem ninguém olhar a tela. O terceiro é defeito: o jogo
executa, responde API e não põe um pixel na tela — e passava despercebido havia quantas sessões
ninguém sabe. **Investigado na mesma sessão, e a causa está identificada.** O Prey Evil não chama desenho
nenhum: o relatório lista onze métodos de GL e **nenhum** `DrawArrays`, `DrawElements`, `glClear`
ou `eglSwapBuffers`. Ele prepara matrizes e texturas, e para — e a tela preta é consequência:
não há o que mostrar.

O motivo está na lista de classes que ele pede e não temos, e **as seis são extensões de GL/EGL**:

| Classe | Nome | Header |
|---|---|---|
| `0x0103d8de` | `AEEIID_GLES10EXT` | `AEEGLES10Ext.h` |
| `0x0103d8eb` | `AEEIID_GLES11EXT` | `AEEGLES11Ext.h` |
| `0x0103def1` | `AEEIID_GLES11EXTPAK` | `AEEGLES11ExtPak.h` |
| `0x0103d8ef` | `AEEIID_EGLGETCOLORBUFFER` | `AEEEGLGetColorBuffer.h` |
| `0x0103d8f0` | `AEEIID_EGLGETPOWERLEVEL` | `AEEEGLGetPowerLevel.h` |
| `0x010426e3` | `AEEIID_EGLOESSWAPINTERVAL` | `AEEEGLOESSWAPInterval.h` |

O jogo particiona o caminho de desenho pelo que existe: sem as extensões, ele não desenha.

**E a correção é menor do que parece.** O motor **já implementa** essas funções —
`eglGetColorBufferQUALCOMM`, `SwapIntervalOES` e as outras da `EGL_QUALCOMM` vivem em
`machine/egl.rs` —, mas as expõe por `eglGetProcAddress`. O jogo as pede por `CreateInstance`, com
o **ClassID**, e recebe nulo. Falta registrar os seis ClassIDs como interfaces, com os slots na
ordem dos headers acima — o mesmo caminho que o `IFont` e o `IGraphics` já trilharam, e com a mesma
verificação: depois de registrar, **a contagem de cores do Prey Evil sai de uma**.

A tabela do `IGLES11Ext` já está lida, e são **15 slots**: os três de `IQueryInterface` (`AddRef`,
`Release`, `QueryInterface`) e, na ordem do `AEEGLES11Ext.h`, `CurrentPaletteMatrixOES`,
`LoadPaletteFromModelViewMatrixOES`, `MatrixIndexPointerOES`, `WeightPointerOES`, `DrawTexsOES`,
`DrawTexiOES`, `DrawTexxOES`, `DrawTexsvOES`, `DrawTexivOES`, `DrawTexxvOES`, `DrawTexfOES`,
`DrawTexfvOES`. Os `DrawTex*` são os que interessam a um jogo que monta o quadro em textura — que é
o caso do Prey Evil, com 16.746 `BindTexture` e nenhum desenho.

**Feito, e medido — o mecanismo está confirmado.** A `IGLES11Ext` foi registrada na fábrica
(`shell_create_instance`) e no `QueryInterface` **do objeto EGL**, que é por onde o jogo pergunta
(a primeira tentativa só na fábrica não mudou nada, e foi o relatório que disse: a classe continuava
na lista). Depois do registro:

```text
antes:  classes que faltam: 0x0103d8de 0x0103d8eb 0x0103d8ef 0x0103d8f0 0x0103def1 0x010426e3
        onze métodos de GL, nenhum desenho, tela de uma cor, "roda" a 3287%

depois: 0x0103d8eb **saiu da lista**
        o jogo passa a fazer sprintf/strlen/strncmp/malloc em volume — outra fase
        e quebra no **próximo** portão: acesso inválido a 0x0, de 0x161d8
```

Ou seja: **entregar uma interface move o jogo um portão adiante**, e o que ele chama em seguida é
o ponteiro nulo de uma das outras cinco.

E foi o que aconteceu, uma a uma, cada uma medida:

| Portão | Interface | Slots | Depois dela |
|---|---|---:|---|
| 1 | `IGLES11Ext` | 15 | sai da lista; o jogo muda de fase (`sprintf`/`malloc` em volume) |
| 2 | `IGLES10Ext` | 4 | sai da lista |
| 3 | `IEGLGetPowerLevel` | 4 | sai da lista |
| 4 | `IEGLOESSwapInterval` | 5 | sai da lista |
| 5 | `IEGLGetColorBuffer` | 4 | sai da lista — **resta uma** |

A verificação é sempre a mesma, e é o que torna isto um procedimento e não uma aposta: a classe sai
da lista de "pedidas e não temos", e o jogo quebra em outro lugar — no mesmo PC enquanto é a mesma
porta, e num PC novo quando a porta muda.

| 6 | `IGLES11ExtPak` | 30 | **a lista de classes faltantes ficou vazia** |

As seis foram entregues, e o relatório do Prey Evil **não tem mais nenhuma classe faltando** — o
levantamento por classe, que é o que diz "o jogo pediu algo que não temos", está limpo.

**E o que sobrou já está identificado.** O jogo ainda quebra no mesmo ponto — `0x161d8` chamando
`0x0` —, e agora não é classe ausente: é um ponteiro nulo que ele guardou antes. Quem entregou a
pista foi o **log do próprio jogo**, nas últimas linhas antes da falha:

```text
*dbgprintf-4* ..\..\..\common\sharedgl\gamepadmgr.cpp:277
Creating USB Joystick interface
*dbgprintf-4* ..\..\..\common\sharedgl\gamepadmgr.cpp:319
1 Joysticks connected
```

O gerenciador de joystick da Qualcomm pede a interface `IJoystick` ao `CreateInstance`, recebe
nulo, guarda — e o primeiro `Read` cai no vazio. E a interface é **pequena**, com os slots no
`sdk/inc/AEEJoystick.h`:

```c
AEECLSID_IJOYSTICK1 = 0x01021c2b        // e IJOYSTICK2 = 0x01021dac
  INHERIT_IQueryInterface(IJoystick);
  int (*SetParm)(IJoystick *po, int16 nParmID, int32 p1, int32 p2);
  int (*GetParm)(IJoystick *po, int16 nParmID, int32 *pP1);
  int (*Read)(IJoystick *po, int16 *px, int16 *py);
```

Seis slots, e o `Read` tem implementação **de verdade** disponível: é o mesmo estado do Z-Pad que
o `IHIDDevice::GetPositionState` já entrega.

**Implementado — e a falha não era esta.** O `IJoystick` está no motor, com os seis slots e o
`Read` entregando o eixo do Z-Pad na faixa do console, e o Prey Evil quebra **no mesmo ponto**, com
o mesmo log. Fica registrado o que isso ensina: o log dizia "1 Joysticks connected", e isso não é
prova de que a interface faltasse — o gerenciador fala isso **depois** de enumerar o HID, que nós
atendemos. A pista estava a duas linhas de distância e eu li a errada.

**A pista de agora, essa sim, veio do próprio relatório:**

```text
open falhou: <cache>/…/mod/276154/udata/save.dat (No such file or directory)
OpenFile "udata/save.dat"  (0x1002bcb4 0x1 0xf0003008) -> 0
```

O jogo abre o **save** com modo `OFM_READ` (1) numa primeira execução, quando o arquivo ainda não
existe, recebe zero e segue. É o mesmo padrão do Double Dragon, um passo adiante: lá era o
`OFM_CREATE` que falhava por falta de diretório; aqui o arquivo não existe porque é o primeiro
save, e a pergunta é o que o console devolve nesse caso — e o que o jogo faz com a resposta.
**Medido, e o nosso comportamento está certo.** O `AEEFile.h` diz o que o console faz:

```text
_OFM_READWRITE and _OFM_APPEND will not create a file.
_OFM_CREATE will only create a file if it did not exist prior to the IFILEMGR_OpenFile call.
```

Ou seja: o console **também** devolve nulo para um `OFM_READ` num arquivo que não existe. Não é
defeito nosso — é o jogo seguindo com o ponteiro vazio. A pergunta que sobra é **onde** ele usa
esse nulo, e para isso o relatório não bastava: é o rastreio.

**A varredura ganhou `ZEEBX_ROM_TRACO`** — `1` guarda as últimas chamadas, ou um filtro de texto
(`ZEEBX_ROM_TRACO=IFile`). Sem esta opção o rastreio existia no motor e ninguém o ligava numa
varredura, que é onde a investigação acontece.

**Os dois funcionam, e o filtro é o mais útil.** Cheguei a anotar aqui que o filtro por família não
dava resultado — **e o erro era meu, na leitura**: usei `sed` do cabeçalho até o fim do arquivo e um
`tail -3`, que pegaram o fim do **log**, não do rastreio. Com `awk` delimitando a seção certa, o
filtro entrega exatamente o que se quer.

E o que ele entregou foi a **sequência final antes da queda**, filtrada por `IHID`:

```text
IHIDDevice::RegisterForButtonEvent     (r1=0x30000650)
IHIDDevice::RegisterForPositionChange  (r1=0x300006d0)
IHIDDevice::GetMinPositionInfo         (r1=0x1002bad8)
IHIDDevice::GetMaxPositionInfo         (r1=0x1002bb3c)
IHIDDevice::GetAxesInfo                (r1=0x1002bba0)
IHIDDevice::GetPositionState           (r1=0x200fff3c)
```

É a calibração do controle, feita pelo gerenciador de joystick do jogo, e **todas respondem
SUCCESS com as estruturas preenchidas** (`write_axis_range`, `write_position_info` com
`POSITION_INFO_WORDS` e `AXIS_SLOTS`). A falha, portanto, está no que o jogo faz **depois** disso —
a tabela de entradas de 28 bytes que o rastreio sem filtro mostrou. É daqui que a próxima sessão
continua: essas seis chamadas são o último contato do jogo com o emulador antes de quebrar.

**E a varredura ganhou o instrumento que faltava para olhar a tabela.** `ZEEBX_ROM_DESPEJO=0xADDR:BYTES`
despeja memória no relatório, em hexadecimal, com o endereço de cada linha:

```bash
ZEEBX_ROM=… ZEEBX_ROM_DESPEJO=0x10038600:128 cargo test --release varredura -- --nocapture
```

O endereço muda de execução para execução (é heap do guest), então o despejo é lido **no fim**, com
o jogo já parado — é quando a tabela está pronta.

Lido, ele mostra o que o rastreio prometia: registros **regulares de 28 bytes**, e um marcador
`ff ff` que aparece de vez em quando, no meio da sequência:

```text
0x10038600  21 0f 18 fe ff 00 0f 28 0f 28 00 58 0f 28 00 27
0x10038660  0f 78 00 39 0f 18 0f 78 ff ff 00 27 0f 78 00 39
```

Os bytes não têm cara de ponteiro de função (nenhum zero, nenhum endereço de módulo), então o nulo
que o jogo chama **não sai daqui direto**.

**Sai de um campo a `+0x274` de um objeto, e isso está decodificado.** O despejo de memória também
serve para ler **código**: com `ZEEBX_ROM_DESPEJO=0x161b0:64` sai a instrução que quebra, e as
quatro palavras à volta dela são legíveis à mão:

```text
0x161c8   LDR r0, [r6, #8]        ; o objeto
0x161cc   LDR r1, [r0, #0x274]    ; o ponteiro de função no campo +0x274  ← veio nulo
0x161d0   MOV r0, sp
0x161d4   BLX r1                  ; ← a chamada que quebra
0x161d8   (retorno)
```

A cadeia é curta e aponta para um lugar só: **o objeto em `[r6+8]` tem o campo `+0x274` nulo**.

**Medido, e o nulo está lá.** Com `r6 = 0x1000110c` no instante da falha, o objeto é `0x10001114`, e
o despejo mostra as duas pontas:

```text
0x10001114   d8 19 00 10  …      → vtable 0x100019d8, **no módulo**: é estrutura do próprio jogo
0x10001388   00 00 … (32 bytes)  → o campo +0x274, todo zero: é ele que o BLX chama
```

**E o cruzamento com o rastreio diz qual campo é.** A última coisa que o jogo fez antes de quebrar
foi a calibração do controle, incluindo `RegisterForPositionChange` — o registro do callback de
posição. O campo `+0x274` é esse callback, e ficou sem ser gravado.

**A hipótese de reentrância foi levantada e derrubada na mesma sessão — e vale registrar por quê.**
A ideia era: o `RegisterForPositionChange` levanta o sinal durante a própria chamada de registro, e
o callback poderia rodar antes de o jogo gravar o ponteiro. A leitura do escalonamento refuta:
`raise_input_signal` só **enfileira** (`pending_signals`), e a fila é drenada no início da **volta
seguinte** (`bombeia_fluxos_pcm` e companhia, em `machine/signal.rs`), depois de a chamada de API ter
retornado inteira. Não há reentrância, e o aviso antecipado — que existe porque faz o Ridge Racer
calibrar sem ninguém encostar no analógico — não tem culpa.

Então o campo `+0x274` fica nulo **pela lógica do próprio jogo**, ou pertence a outro objeto que não
o que o despejo pegou. O que sobra é engenharia reversa do jogo, com cinco pontos de partida
medidos: o PC (`0x161d8`), a pilha (`0x43494 0x4f300 0x3af54 0x43d5c`), o objeto (`0x10001114`,
vtable `0x100019d8`), o campo (`0x10001388`) e a tabela de 28 bytes (`0x10038600`).

E o que ele mostrou foi o padrão exato antes da queda: uma **tabela sendo construída**, entradas de
28 bytes (`malloc 0x1c` seguido de `memmove 0x1c`), num laço, com as origens a 28 bytes de
distância (`0x1003869a`, `0x100386b6`, `0x100386d2`, …) — e **nenhuma chamada de API depois disso**.
A falha é no código do próprio jogo, chamando um ponteiro que ele montou (ou leu de uma tabela),
não um ponteiro nosso.

É onde esta sessão parou: o que sobra é engenharia reversa do jogo, com o rastreio na mão e o PC
(`0x161d8`), a pilha (`0x43494 0x4f300 0x3af54 0x43d5c`) e as origens da tabela (`0x1003869a`) como
ponto de partida.

O `IGLES11ExtPak` responde com o tratamento das outras extensões gráficas — "consegui" ao que não
muda o traço, identificador `1` para os `Gen*OES` (o alvo aqui é um só) e `GL_FRAMEBUFFER_COMPLETE`
(0x8CD5) para a consulta de completude.

**E o número da classe já aparece no motor por outro caminho.** No BREW, o ClassID e o IID são o
mesmo valor: `AEEIID_GLES_IMAGEON_EXT` (`0x01058546`) já é atendido em `machine/egl.rs` pela rota de
**função** (`eglGetProcAddress`), e o Prey Evil pede as extensões dele pela rota de **objeto**
(`ISHELL_CreateInstance`). Falta o registro na fábrica, em `shell_create_instance`, com a tabela de
slots acima — e o handler da rota de função serve de referência para o que cada slot responde.

### Dois jogos mudos, e o que os calava

O relatório da varredura lista os sons que o decodificador recusou. Dois jogos apareciam ali, e a
medição de áudio — pico, rms, contínuo e salto entre amostras — mostrou **pico 0,000 nos dois**:
silêncio absoluto, não "sem som por enquanto".

Investigar começou por fazer a recusa **dizer o formato e os primeiros bytes**. Duas linhas
resolveram os dois casos:

```text
Turma da Mônica   recusado (formato desconhecido, 86 sons, 4f 67 67 53 ...)   ← "OggS"
Peggle            recusado (audio/mpeg, 7 sons, 49 44 33 03 00 00 00 00 ...)   ← "ID3"
```

- **Ogg/Vorbis**: o `symphonia` já sabia decodificar; faltava ligar a feature. 86 sons viraram
  música — pico 0,586 aos trinta segundos virtuais, contra 0,000 antes.
- **MP3 com etiqueta ID3 que mente**: o Peggle entrega etiqueta dizendo **zero byte** de tamanho e
  trinta mil de conteúdo. Quem pula pelo campo declarado procura o quadro no lugar errado e recusa
  o arquivo inteiro. Agora a busca é pela **sincronia do quadro** (`0xFF` e três bits altos), que é
  o que o formato garante — sete trilhas voltaram: pico 0,356 contra 0,000.

A lição vale para a próxima: uma recusa que não diz **o que** chegou custa uma investigação inteira
dentro do jogo. Dizendo o formato e os bytes, foram duas linhas de diagnóstico e vinte de correção.

No corpus inteiro, depois das duas correções: **zero sons recusados** (eram dois) e dezesseis jogos
com som medido nos seis segundos da varredura. O Turma da Mônica entra nessa conta só depois de
trinta segundos virtuais — a varredura curta o deixaria de fora, e por isso o número dele foi
medido à parte.

### Onde o tempo vai, medido

Com `ZEEBX_ROM_PERFIL=1`, a varredura grava o custo real por método de API. Nos dois jogos mais
pesados do acervo:

| Jogo | Tempo em chamadas de API | O que domina |
|---|---:|---|
| Crash Nitro Kart 3D | 462 ms | `eglSwapBuffers` 415 ms = **89,8%** |
| Zeebo Extreme Rolima | 958 ms | `memset` 520 ms, `eglSwapBuffers` 257 ms |

Os 415 ms do Crash são **3,5 ms por quadro apresentado** (118 quadros), e ali dentro está o
rasterizador software desenhando a cena — trocar o buffer é o mesmo que desenhar. Ainda assim o
jogo roda a **1039%** da velocidade do console: o rasterizador é o maior custo isolado e **não é
gargalo** para o que existe hoje. O que o render em hardware muda é a **fidelidade** (estado de GL,
texturas comprimidas, blend), não a velocidade deste acervo.

O `memset` do Rolima custava 731 ms porque o `helpers` **alocava um `Vec` do tamanho pedido em cada
chamada**. Com limpeza em blocos de um buffer de pilha: 520 ms, e o jogo de 247% para 267%.
| Render em hardware (`SET_HW_RENDER`) | **encaixado, provado até onde dá sem frontend** | rasterizador verificado contra o de software; o motor desenha no framebuffer do frontend; as features `gl`/`gpu` separadas (0 dependências de host, medido em CI nos 6 alvos); o core pede o contexto, monta o `glow::Context`, desenha no FBO e volta ao software se falhar. O desenho em si depende do RetroArch |

### O rasterizador da placa desenha o mesmo quadro, medido

Antes de ligar `SET_HW_RENDER` no core, era preciso saber se o caminho de GPU desenha **a mesma
imagem** que o de software — senão a troca seria sair de um caminho medido para um que ninguém
olhou. O teste `os_dois_rasterizadores_desenham_o_mesmo_quadro` roda o mesmo conteúdo pelo mesmo
tempo virtual nos dois e compara pixel a pixel, **sem janela** (o contexto fora de tela existe
justamente para isso: medir).

```bash
ZEEBX_TESTE_ROM="roms/jogo.zip" ZEEBX_TESTE_MS=3000   cargo test --features gpu os_dois_rasterizadores -- --nocapture
```

| Jogo | Pixels diferentes | Diferença média | Pior pixel |
|---|---:|---:|---:|
| Double Dragon | **0,00%** | 0,000 | 0 |
| Crash Nitro Kart 3D | **0,00%** | 0,000 | 0 |
| Zeebo Sports Peteca | 2,62% | 0,034 | 3 de 63 |

Dois jogos saem **idênticos**, e o terceiro difere só em arredondamento de meio nível — o que a
comparação cobra é que desenhem a mesma imagem, não que sejam bit a bit iguais. Com esta conta
feita, o que falta no item 5 é o encanamento: negociar o contexto com o frontend na
`RETRO_ENVIRONMENT_SET_HW_RENDER` e entregar o `glow::Context` que o motor já aceita.

Enquanto o encanamento não existe, dá para **ver** os dois rasterizadores lado a lado pela linha de
comando, sem interface — os mesmos dois comandos, mudando só a flag da placa:

```bash
cargo run --release -- sessao "roms/Crash.zip" --seconds=3 --dump=software.bmp
cargo run --release -- sessao "roms/Crash.zip" --seconds=3 --placa --dump=placa.bmp
```

Os dois `.bmp` são o mesmo instante virtual do mesmo jogo, um desenhado no processador e outro na
placa. É a conferência que qualquer pessoa faz sem escrever código, e foi ela que o teste acima
automatizou.

### O que já passou pelo caminho do core

O teste `a_abi_do_core_roda_uma_rom` exercita o core como o RetroArch o exercita — `retro_init`,
`retro_load_game`, quadros, `retro_unload_game` —, e conta o que chega ao frontend. Medido hoje:

| Conteúdo | Quadros | Amostras estéreo | Jogos ao lado |
|---|---:|---:|---:|
| Crash Nitro Kart 3D (`.zip`) | 360 | 267.668 | 63 |
| Double Dragon (`.zip`) | 360 | 268.729 | 63 |
| Double Dragon (`.7z`) | 300 | 224.669 | — |
| Peggle (`.zip`) | 300 | 229.080 | — |
| Z-Wheel (`.zip`) | 300 | 220.300 | 63 |

```bash
ZEEBX_CORE_ROM="roms/Crash.zip" ZEEBX_CORE_QUADROS=180 \
  cargo test -p zeebx-libretro -- --nocapture
```

Cada linha é uma coisa que deixou de ser suposição: o 3D entrega quadro e som pelo core, o `.7z`
abre pelo core, e a Z-Wheel vê os 63 jogos pelo core. O que **não** está nesta tabela é o desenho
na placa: para esse é preciso um frontend de verdade, e é o que falta no item 5.

### A lacuna de GL que o levantamento achou

O relatório da varredura tem uma seção **"GL atendido sem fazer nada"**: chamada que o rasterizador
aceita com sucesso e ignora. Nas 62 ROMs ela apontava **uma só** chamada, em seis jogos — o
`glPixelStorei`. Não é ruído: o GL alinha cada linha de textura num múltiplo do alinhamento pedido
(4 por padrão), e o que sobra é enchimento. Ignorando a chamada, uma textura cuja largura **em
bytes** não é múltipla de quatro chega com as linhas deslocadas — a imagem sai embaralhada em
diagonal, sem uma linha de aviso.

Implementado: o alinhamento vive no `Machine` (é propriedade da memória do guest, não do
rasterizador), a leitura dos texels compacta as linhas num único acesso, e `arredonda_para` tem
teste. Conferido depois: Double Dragon e Galaxy on Fire **deixam de aparecer na seção**, e continuam
rodando. É o tipo de defeito que o render em hardware resolveria de graça — e que aqui custou vinte
linhas, sem trocar de rasterizador.
| Save states | **falta** | o core **declara** que não tem (`size` 0, `serialize` falso), que é o critério de aceite |
| `.7z` | feito | `/tmp/dd.7z` → `estado: roda`; limites iguais aos do zip; e **pelo caminho do core**: 300 quadros entregues ao frontend |
| Ciclo da Z-Wheel no RetroArch | **parcial** | o motor é medido pela varredura; o laço do core que troca de sessão não tem teste automático |
| Capas e No-Intro | **falta** | depende de conta e de envio externo |

### O que a rodada de CI ensinou

Três defeitos que só apareceram quando os testes passaram a rodar até o fim, e que valem para a
próxima vez:

1. `cargo test` **nunca** tinha compilado o alvo de teste do binário: o `main.rs` importava
   `zeebx::varredura` sob `#[cfg(test)]`, e ali a biblioteca é compilada sem esse `cfg`. Rodar
   `cargo test --lib` escondia isso.
2. Dois testes de VFS comparavam a caixa exata do caminho, e o APFS do macOS **não distingue
   caixa**: o arquivo pedido já existe, e não há duas grafias para comparar.
3. `continue-on-error: ${{ matrix.experimental }}` faz o GitHub **rejeitar o arquivo de workflow
   inteiro** — o run aparece como falha **sem nenhum trabalho**. `actionlint` e um leitor de YAML
   não acusam: o diagnóstico é o run vazio.

## Publicação do core

O core é publicado junto com os instaladores, na mesma tag. O trabalho `core` do `release.yml`
compila e confere os seis alvos (a mesma prova de ABI do `libretro.yml`) e empacota **um `.zip` por
alvo**, com a biblioteca e o `.info` juntos — o RetroArch quer os dois com o mesmo nome na pasta de
cores, e seis arquivos de mesmo nome não cabem soltos numa release.

```text
zeebx_libretro-linux-x86_64.zip      zeebx_libretro-windows-x86_64.zip
zeebx_libretro-linux-aarch64.zip     zeebx_libretro-windows-aarch64.zip
zeebx_libretro-macos-x86_64.zip      zeebx_libretro-macos-arm64.zip
```

Cada sistema empacota com a ferramenta que tem: `zip` no Linux e no macOS, `Compress-Archive` no
Windows — o `zip` **não existe** no runner do Windows, e um `shell: bash` com `zip` falharia num
terço dos alvos no dia da tag.

### Save state: o formato existe, o conteúdo é o que falta

O item 6 pede "serialize/unserialize com formato versionado". O **formato** está feito e testado
(`src/save_state.rs`): assinatura `ZBXS`, versão, tamanho, `crc32` do conteúdo e seções **nomeadas** —
uma seção desconhecida é pulada, que é o que deixa um motor velho ler um estado gravado por um novo.
Oito testes, incluindo os que recusam arquivo cortado, byte trocado, assinatura errada e versão do
futuro (dizendo qual). 

O **conteúdo** é o que falta, e agora está medido em vez de estimado: o `Machine` tem **190 campos**,
dos quais ~180 são estado de verdade. Por tipo:

```text
 50 HashMap   ·  32 u32   ·  19 Vec   ·  16 Option   ·  13 BTreeSet
  8 u64       ·   8 bool  ·   7 VecDeque  ·  5 ArrayPointer  ·  3 BTreeMap  ·  2 Heap
```

Os maiores são tabelas indexadas por **ponteiro do guest**: `objects`, `collections`, `fontes`,
`databases`, `open_files`, `decoders`, `images`, `sounds`, `ciphers`, `widgets`, `streams`,
`threads`, `timers`, `signals`… mais o `heap` e o `objects` (contadores de endereço) e a memória do
guest, que vive no backend de CPU.

**A boa notícia de projeto:** os ponteiros do guest são endereços absolutos, então restaurar a
memória nos mesmos endereços mantém válido **todo** ponteiro guardado nessas tabelas — não há
remapeamento. O trabalho é enumerar as seções e cobrir cada uma com ida e volta, não inventar um
formato de grafo.

**Enquanto não estiver inteiro, o core continua respondendo `retro_serialize_size = 0`**, e há teste
que cobra isso: zero é como o frontend entende "este core não salva". Estado parcial é o que o
critério proíbe, e ele não escaparia por descuido porque a porta é essa.

### O que já entra no estado, e o que falta

Entrou, com ida e volta testada:

- **registradores** (16, na ordem fixa da seção — gravar "todos na ordem do `enum`" deixaria o
  formato refém de alguém reordenar a enumeração);
- **memória de toda região gravável**, lida do núcleo (é lá que a memória vive depois do `reset`,
  e não no mapa do carregador);
- **o livro do heap** (base, fim, primeiro endereço nunca usado, livres e em uso), com duas
  checagens na volta: o heap tem de ser desta máquina, e um endereço não pode estar livre e em uso
  ao mesmo tempo.

Duas decisões que valem registro, porque uma delas eu escrevi errado primeiro:

1. **O corte no heap sai do próprio estado**, e não da máquina de agora. Quem carrega um save state
   carrega o heap que estava lá — eu tinha tratado "o heap do estado é maior" como erro, e é o caso
   normal.
2. **A região de objetos e a de superfícies ainda vão inteiras.** Truncá-las pelo `next` dos
   alocadores delas seria mais barato, mas os livros delas ainda não entram no formato, e um
   tamanho que a volta não pode conferir é pior que um tamanho maior.

### O inventário do que já entra, e o que falta

O `Machine` tem **190 campos**. O estado que já vai e volta, com teste de ida e volta cada:

```text
cpu              registradores, CPSR, relógio virtual, memória de toda região gravável
livros           heap, objetos (com a interface codificada), superfícies
entrada          filas de tecla e de botão, aparelho de cada porta, avisos de aparelho
agenda           timers, retornos registrados e pendentes
tabelas          ~20 mapas e tabelas numéricas (dib, streams, sons, imagens, transparência…)
conteúdo         preferências, parâmetros, IConfig, fontes, páginas, texto decifrado
pixels           superfícies, tela, imagens decodificadas
texto            módulos instalados, enumerações, caminhos de arquivo aberto
parada           o ponto onde o motor parou, variante por variante
resto            GL (vetores de cliente), IGraphics, teclados, threads, quadros do Update
widgets          os três mapas, texto, coordenadas e as três duplas de função
cifra/zip/peek   o estado de biblioteca que tem forma fechada
```

Ficam **fora de propósito**, com o motivo escrito no código: os campos de instrumento e diagnóstico
(`api_time`, `profiling_api`, `tracing`, `fault_*`, `ignored_gl`, `*_log`, `missing_*`, `bad_pointers`),
o `Mixer` do host, o `clock_us` (derivado do contador de instruções), e os **caches do que está em
disco** (`vfs`, `resources`, `interned`, `CargaDeMidia`) — gravar esses seria duplicar o pacote e o
arquivo dentro do save state.

Faltam **três peças**, e nenhuma é mecânica:

| Peça | Tamanho | Por que não entrou ainda |
|---|---|---|
| `gl` — a máquina de estados de GL do guest | 57 campos | ver a classificação abaixo; é a peça mais arriscada do item |
| `decoders` | por objeto | decodificador de imagem no meio de um fluxo: expor o de dentro, ou aceitar perder o que estava em curso |
| `databases` | por objeto | banco SQL aberto — a mesma decisão |

### A máquina de estados de GL, classificada

Os 57 campos do `GlState` **não são um bloco**: são quatro grupos, e cada um pede uma decisão
diferente. Isto foi levantado campo por campo, com o tipo ao lado, para a próxima passada ser
mecânica em vez de arqueologia.

**A — gravar (48 campos).** O estado que as chamadas de GL do jogo mudam e que o desenho seguinte
lê: as três pilhas de matriz (`modelview`, `projection`, `texture_matrix`, 16 floats cada), o modo de
matriz, o viewport e a tesoura (com a crua e a ligada), `surface` e `esticada`, as cores de limpeza e
a corrente, o alvo de textura ligado e as duas unidades, as bandeiras de teste e as funções
(`depth_*`, `blend_*`, `alpha_*`, `cull_*`, `stencil_*`, com os três `stencil_op`), a névoa, a faixa
de profundidade, a máscara de cor, e a iluminação (`lighting`, `color_material`, `lights`,
`material`, `light_model_ambient`, `shade_model`). São números e vetores de tamanho fixo — a mesma
forma das tabelas que já entraram.

**B — gravar com formato próprio (1 campo).** `textures`, com a cadeia de mipmaps de cada uma. É o
que dá volume ao grupo e o que exige cuidado: um nível de mipmap que falte não dá erro, dá listra
na imagem — foi assim que o defeito apareceu quando a cadeia passou a ser usada.

**C — decidir se gravar (3 campos).** `color`, `depth` e `stencil`, os três buffers de 640×480. O
de cor é o candidato mais interessante: ele é **escrito a partir do quadro e lido para apresentar**,
e a tela (`screen`) já entra no estado — se a relação entre os dois for de derivação, gravar os dois
é duplicar 1,2 MB. Os de profundidade e stencil não têm duplicata: sem eles, o teste de profundidade
e a marcação de stencil recomeçam, e o palco da Z-Wheel perde o reflexo. **Esta é a decisão que falta
tomar, e ela se toma medindo quem escreve em quem.**

**D — não gravar, e conferir que estão vazios (4 campos).** `batch`, `pending`, `transformed` e
`sujo` são acumuladores de um desenho **em curso**. O frontend chama o serialize **entre quadros**,
nunca dentro de um `retro_run`, então no instante do save eles estão vazios — e é isso que os torna
dispensáveis. O certo não é confiar nisso: é **recusar** o save se algum deles não estiver vazio, e o
teste cobrir os dois lados.

### O save state está de pé

As três peças entraram, e com elas o **estado completo**. O core deixou de responder zero:

```text
retro_serialize_size   o tamanho do estado, medido agora (7.084.619 bytes com o Double Dragon)
retro_serialize        entrega o que o `_size` mediu, e recusa se não couber no buffer
retro_unserialize      põe de volta, e recusa com o motivo quando alguma seção não bate
```

Duas decisões de encaixe que valem registro:

- **o tamanho medido guarda os bytes.** A ABI chama o tamanho e a gravação em sequência, e o tamanho
  varia com o que o jogo tem em memória — recalcular na gravação daria um estado **diferente** do que
  foi medido, e o frontend teria alocado o buffer pelo número errado;
- **o motor drena a fila de desenho antes de conferir.** Um lote de triângulos esperando a vez não é
  desenho pela metade — é trabalho que ia ser feito no quadro seguinte. Recusar o save por causa dele
  bloquearia o jogador por algo que o motor resolve sozinho. Depois de drenar, o que sobra é o
  desenho interrompido de verdade, e aí a recusa é honesta.

**A prova de ponta a ponta** é o teste `o_save_state_atravessa_a_abi`, e ele faz o caminho que o
RetroArch faz: mede o tamanho, grava, roda mais quadros para o estado **mudar**, carrega, grava de
novo e exige **byte a byte o mesmo arquivo**. Depois estraga um byte e exige que a recusa não mexa na
máquina. Comparar campos seria mais fraco: o que o jogador vê é o jogo continuar do mesmo ponto.

**O que continua fora, por decisão declarada:** os campos de instrumento e diagnóstico, o `Mixer` do
host, o `clock_us` (derivado do contador de instruções), e os caches do que está em disco (`vfs`,
`resources`, `interned`, `cargas_de_midia` — este último **com caminho de recarga**, e é por isso que
não é lacuna).

### O livro dos objetos, e a codificação das interfaces

O `enum Interface` tem **62 variantes, todas sem payload, com discriminante explícito de 0 a 61** —
então o número no arquivo é o próprio discriminante, e não uma tabela paralela que alguém precisa
manter. A volta (`de_codigo`) é escrita à mão porque Rust não desfaz um `as u32`, e há teste que
cobra **as 62**: interface nova sem entrada deixa o teste vermelho antes de um save state ficar
ilegível. É o tipo de coisa que precisa doer no dia em que se mexe, e não no dia em que alguém
carrega um estado.

Com o livro dos objetos no formato, a região deles e a das superfícies passaram a ser **cortadas no
primeiro endereço nunca usado**, como o heap — e o corte só é honesto porque o livro de cada uma
entra no arquivo, de onde sai o tamanho esperado na volta.

| | antes | agora |
|---|---|---|
| estado de uma máquina mínima | 14,07 MB | **2,07 MB** |
| memória mapeada | 84 MB | 84 MB |

Uma armadilha que apareceu no caminho e vale registro: a máquina tem **dois** `Heap` (a memória do
jogo e a região das superfícies). Com nome de seção fixo, o segundo sobrescreveria o primeiro no
arquivo, e o defeito apareceria como "as superfícies voltaram no lugar do heap". As seções passaram a
ter prefixo.

Falta, para fechar: o `ObjectStore` (depende de codificação estável do `enum Interface`), as ~50
tabelas de estado por objeto, o contador de instruções (é o relógio virtual, e o trait do núcleo
ainda não tem como recebê-lo de volta) e as **flags** da CPU — o `CpuBackend` expõe 16 registradores
e o modo Thumb, e não o `CPSR`.

**Ensaio local do passo de empacotamento, porque ele só roda em tag.** Os comandos do workflow,
executados à mão:

```text
zeebx_libretro-linux-x86_64.zip
  zeebx_libretro.info    332 bytes
  zeebx_libretro.so      15.637.608 bytes

extraído: a biblioteca carrega e responde api_version 1
```

Dois arquivos com o **mesmo nome**, e a biblioteca funcional depois de sair do `.zip` — que é
exatamente o que o RetroArch precisa para aceitar o core.

**E os nomes dos artefatos não colidem**, que era o outro risco daquele passo: o job da release
baixa tudo para um diretório só e publica `pacotes/*`, então dois arquivos com o mesmo nome
derrubariam a publicação. Conferido: são **11 artefatos** — cinco instaladores (dois `.deb`/AppImage
no Linux, um `-setup.exe`, dois `.dmg`) e seis `.zip` do core, um por alvo, todos com nome distinto.
Sem os zips por alvo, seriam seis `zeebx_libretro.so` iguais.

## O que falta, com o mapa de cada item

### Item 5 — render em hardware: só o encanamento

**O que já existe:** o rasterizador da placa, verificado contra o de software (idêntico em Double
Dragon e Crash; 2,6% de arredondamento no Peteca); o contexto fora de tela para medi-lo sem janela;
`Session::start_with(..., placa, Some(Arc<glow::Context>), ...)`, que já aceita o contexto do
frontend.

**O que falta, em ordem:**

### O struct da placa, verificado por deslocamento

Antes de pedir o teste do Rafael, conferi a suposição que faria o render em hardware falhar **em
silêncio**: o `retro_hw_render_callback` que o core preenche. Se o meu struct tivesse um campo a
mais ou a menos, `get_current_framebuffer` e `get_proc_address` seriam lidos no lugar errado — e o
sintoma seria "não funciona", sem nada no log.

O `libretro.h` do repositório resolveu a dúvida: nesta revisão o `context_destroy` vem **depois** do
`cache_context`, e não em terceiro lugar como em outras. Os deslocamentos do meu struct batem com
ele, e agora há teste: `os_deslocamentos_do_struct_da_placa_batem_com_o_libretro_h` cobra 8, 16, 24,
32, 36, 40, 44 e 48 para os campos que usamos.

A conferência também achou uma lacuna real: eu **não** preenchia o `context_destroy`. Agora ele
existe e tem efeito — quando o frontend troca o contexto de vídeo (trocar de driver, por exemplo),
o core descarta o `glow::Context`, **recria a sessão em software** e avisa na tela. Sem isso, a
sessão continuaria no rasterizador de placa chamando funções de GL que já não existem.

0. ~~Separar o backend de placa da criação de contexto.~~ **Feito**: o motor tem duas features —
   `gl` (o desenho, que **só** precisa do `glow`, sem nada de host) e `gpu` (abrir contexto nosso,
   que traz o `glutin` e linka EGL/GLX). Medido: `cargo tree --features gl` tem **0** glutin e o
   `glow` presente; `--features gpu` tem 3. Era isto que faltava para o core poder usar a placa: um
   core não pode linkar biblioteca de interface do host, e é isso que a checagem de `ldd` do
   `libretro.yml` cobra.
1. ~~Ligar a feature `gl`, montar o contexto e negociar o `SET_HW_RENDER`.~~ **Escrito e
   compilando**, com a válvula de segurança: se o frontend aceitar e não cumprir, ou se a sessão na
   placa falhar, o core **volta ao software** e diz por quê no log. `troca_para` também nasce na
   placa, para a Z-Wheel abrindo um jogo não devolver o desenho ao processador.
2. ~~Fazer o motor desenhar no framebuffer do frontend.~~ **Feito e verificado**: o motor tem
   `desenha_no_fbo`, o teste `o_motor_desenha_no_framebuffer_do_frontend` cria um framebuffer
   próprio, manda o motor desenhar nele e lê os pixels **dele** — nos dois jogos medidos, **0 de
   307.200 pixels** ficaram com a cor de nascença, ou seja, o desenho foi todo para o framebuffer
   de fora, e não para o interno.
3. Trocar o `retro_video_refresh` de quadro por `RETRO_HW_FRAME_BUFFER_VALID` — sem isso o
   RetroArch recebe pixels que não são os do FBO.

O lado do motor já está pronto para receber: `Session::desenha_no_fbo(Option<u32>)` é público, e
`Some(0)` significa o framebuffer padrão do frontend. O que sobra no core é ligar os fios.

**Uma armadilha de ordem, para não gastar uma sessão descobrindo:** o `libretro` só entrega um
contexto de GL **válido** depois de chamar o `context_reset` do struct que o core preencheu. Quem
chama `SET_HW_RENDER` é o core (dentro do `retro_load_game`, antes de criar a sessão), e quem avisa
"o contexto está pronto" é o frontend, **depois**. Ou seja: a sessão de placa não pode ser criada
junto com a negociação — ela precisa esperar o `context_reset`, ou o primeiro `retro_run`. As duas
saídas possíveis são criar a sessão dentro do `context_reset`, ou guardar o pedido e criá-la no
primeiro `retro_run`, quando o contexto já está corrente. E `get_current_framebuffer` é consultado
**por quadro**, no `retro_run`, porque o framebuffer pode mudar.

**Como verificar:** o teste `os_dois_rasterizadores_desenham_o_mesmo_quadro` continua valendo como
régua; o que muda é por onde o quadro sai.

### Item 6 — save states: por que ainda não

O critério de aceite do plano é "ausência de save state é declarada corretamente, **sem estado
parcial**", e é o que o core faz (`retro_serialize_size` devolve 0, `retro_serialize` é falso). Um
estado parcial seria pior que nenhum: a memória do guest e os registradores voltariam, e a mesa de
objetos do emulador não — o jogo retomaria chamando APIs com identificadores que não existem mais.

**O que um estado completo exigiria, contado:** o `Machine` tem **183 campos**. Classificados por
nome, são:

| grupo | campos | o que fazer |
|---|---:|---|
| instrumentos (`bad_pointers`, `calls`, `api_time`, `assumptions`, `debug_*`, contadores de recusa) | 13 | **nada** — não são estado |
| tabelas de objetos (`bitmaps`, `canvases`, `databases`, `decoders`, `collections`, `egl_surfaces`, …) | 18 | descrever cada objeto vivo e recriá-lo na carga |
| agendamento (`*_pendente`, `current_thread`, `delivered`) | 13 | serializar a fila |
| escalares e contadores (`clock_us`, `epoch_seconds`, séries) | 10 | um `u64` cada |
| o resto (estado de desenho, EGL, configuração em vigor, listas de diagnóstico) | ~129 | separar o que o jogo vê do que é do emulador |

**O critério que faz o escopo encolher: instrumento não é estado.** Tudo o que existe para o
relatório — ponteiros recusados, contagem de chamadas, tempo por método, hipóteses — fica de fora
do save state, senão carregar um estado carregaria também o histórico de depuração de outra sessão,
e a linha de base da varredura passaria a depender de quando o jogo foi salvo.

**E o critério que faz o escopo crescer:** as 18 tabelas de objetos precisam de uma **descrição**
que baste para recriar cada objeto — arquivo aberto e deslocamento, superfície e formato, fluxo de
mídia e posição, consulta SQL e cursores. É aí que está o trabalho, e é aí que um estado parcial
mentiria.

### Item 8 — ciclo da Z-Wheel: o que já está medido

A varredura **não** consegue exercitá-lo: doze segundos com e sem manche diferem em mil instruções
de 133 milhões, e a roda não desenha nada naquele caminho (zero quadros, zero texto). A conclusão
prática é que **só o RetroArch responde** — não vale gastar outra sessão tentando por aqui.

A **precondição**, essa sim, está medida pelo caminho do core: ao abrir a Z-Wheel, o core acha
**63 jogos** ao lado do conteúdo (o levantamento por ClassID, com deduplicação entre o `.zip` e a
cópia extraída). Isso separa dois sintomas que se parecem: **"a roda abre vazia" não é falha de
descoberta** — ela soube de todos os 63.

**E a entrada chega ao guest, medido.** O teste `a_entrada_do_retropad_chega_ao_guest` aperta um
botão do RetroPad pelo caminho do core e conta as imagens distintas que o jogo devolve:

```text
Pac-Mania:   botão START →  2 imagens distintas em 60 quadros (o jogo respondeu)
Peteca:      botão START → 28 imagens distintas em 60 quadros
```

Isso fecha a outra hipótese que estava aberta: quando a Z-Wheel não reagiu ao manche no teste
headless, eu não sabia se era o caminho de entrada do core ou o estado da roda. **É o estado da
roda** — a entrada comprovadamente chega ao guest, em dois jogos. O que falta na Z-Wheel é o
frontend de verdade, que é o que o item 8 pede.

### Item 9 — capas e No-Intro: depende de conta

Local está feito, e remedido em 22/09/2026:

```text
playlist  Mobile - Zeebo.lpl        62 entradas, 0 sem arquivo
capas     Named_Boxarts            116 arquivos (58 títulos, em .jpg e .png)
          Named_Titles              44 arquivos (icone do .mif, provisorio)
banco     Mobile-Zeebo-No-Intro-libretro.dat   12 372 bytes, 57 jogos, versao 2026.08.01
RDB       Mobile - Zeebo.rdb       instalado, e o que faz o "Scan Content" casar por hash
```

O que falta é publicar no repositório de thumbnails do Libretro (o diretório tem de se chamar
`Mobile - Zeebo`, que é o nome do banco — as capas já estão nesse formato e nesse nome) e enviar os
cinco títulos fora do No-Intro. **Os dois precisam de conta e de conferência humana**, e é decisão do
Rafael: nada foi publicado e nenhum repositório externo foi criado.

**O formato estava errado para o destino, e isso foi corrigido.** Os repositórios de thumbnails do
RetroArch aceitam **só PNG**, e as capas que a Z-Wheel entrega são JPEG — os cinquenta e oito
arquivos seriam recusados no dia do envio. O catálogo ganhou `--png`, que converte na hora de
gravar (com o Pillow; sem ele, grava o original e avisa), e a geração conferida:

```bash
python3 ferramentas/catalogo.py --roms ROMS --saida saida --png --zwheel "Z-Wheel.zip"
# capas oficiais: 58 de 62  ·  png: 58  ·  jpg: 0
# Action Hero 3D ….png: PNG image data, 170 x 220, 8-bit/color RGB
```

**E os títulos fora do No-Intro saem com os hashes prontos.** O que a proposta pede é nome,
tamanho, CRC32, MD5 e SHA1 do arquivo que o DAT hasheia — e para um título que não está no DAT
esse arquivo é decisão de quem envia. A ferramenta grava `<saída>/fora-do-dat.txt` com **todos** os
arquivos de `mod/<id>/` e os três hashes de cada um, e diz no cabeçalho que a escolha é humana:
adivinhar aqui devolveria a proposta recusada, e o trabalho seria o dobro.

```text
## Bad Dudes vs. DragonNinja (Brazil) (Es,Pt)
pacote: Bad Dudes vs. DragonNinja (Brazil) (Es,Pt).zip  (1369145 bytes)
crc32 do pacote: 56020280   sha1: 2E E9 …
  mod/279888/baddudes.eng
    tamanho: 1456  crc32: AD9831B6  md5: 82A42D24…  sha1: E4DCEE01…
```

E o mesmo conteúdo sai em **formato DAT**, que é o que a proposta pede de verdade — cinco blocos
`game (`, com o nome do arquivo na convenção do banco (a pasta do módulo e o arquivo, sem barra):

```text
game (
	name "Bad Dudes vs. DragonNinja (Brazil) (Es,Pt)"
	region "Brazil"
	rom ( name "mod279888baddudes.mod" size 3034964 crc DE1F72C0 md5 3567F3EE… sha1 BB5781BE… )
	… (todos os arquivos do módulo)
)
```

Um `game` com vários `rom` é válido no formato, e é o certo aqui: **qual dos arquivos é o dump é
decisão de quem mantém o banco**. A proposta leva todos, com os hashes, em vez de apostar num — e
aposta errada, nesse caso, custa uma rodada inteira de ida e volta.

## Ordem de implementação

1. Inventariar toda E/S de core e definir `StorageFs`/`GuestFile`; decidir SQLite VFS ou staging
   transacional antes de prometer caminhos não-nativos.
2. Criar `StoragePaths`, `ContentIdentity` e `ContentLayout` sobre essa abstração.
3. Mover configuração e leitura de `.mif` para módulos neutros.
4. Refatorar archive, machine, SQL, widgets, fontes, mídia, shell e VFS para receber storage
   explícito; manter `std::fs` somente em CLI/UI/testes desktop.
5. Implementar VFS com base read-only, overlay gravável, tombstones e device compartilhado.
6. Extrair driver de frame virtual determinístico de `Session`.
7. Criar `src/lib.rs`, features e motor sem dependências desktop; converter raiz em workspace.
8. Criar `frontends/libretro/` como pacote `cdylib`, vendorizar `libretro.h`, gerar bindings e
   implementar todos os exports base, incluindo VFS v3.
9. Implementar `.mod`/ZIP, RGB565, áudio PCM16, Dragon/Z-Pad e RetroPad.
10. Adicionar `SET_CONTROLLER_INFO`, teclado USB com fila AVK e Boomerang estático.
11. Integrar sensor Libretro e calibração do Boomerang; validar fallback analógico depois.
12. Adicionar testes determinísticos, VFS, COW, ZIP malicioso e persistência cruzada.
13. Integrar e testar RetroArch x86_64.
14. Adicionar AArch64 nativo/CI.
15. Implementar 7z com corpus real e limites de segurança.
16. Só depois: Core Options, suporte SAF/Android completo, mouse guest medido, HW render e save
    states.

## Critérios de aceite do MVP

- `frontends/libretro/` é pacote independente e não traz UI desktop como dependência;
- core x86_64 carrega em RetroArch;
- `.mod` e `.zip` iniciam applet e exibem framebuffer;
- Dragon, Z-Pad, Boomerang e teclado USB aparecem ao guest conforme aparelho selecionado;
- input RetroPad chega ao Z-Pad;
- teclado Libretro entrega aperto e soltura AVK sem executar guest dentro do callback;
- Boomerang recebe amostra de acelerômetro do frontend, ou informa claramente fallback/ausência;
- áudio estéreo chega ao frontend;
- saves relativos sobrevivem a unload/reload;
- `fs:/` é comum entre títulos;
- limpar `cache/` não apaga saves nem `aparelho`;
- ROM e cache de conteúdo não são modificados por execução;
- ZIP malicioso não escreve fora de `zeebx/`;
- conteúdo, save e cache passam por `StorageFs`/VFS; Linux nativo é validado no MVP;
- caminhos não-POSIX (SAF/Android) só são anunciados após a solução SQLite/VFS e testes reais;
- `GET_SAVE_DIRECTORY` ausente/não gravável falha sem criar save temporário enganoso;
- concorrência no mesmo `aparelho/` é recusada no backend nativo quando lock existe, e documentada
  como não suportada onde VFS não oferece exclusão mútua;
- Z-Wheel pode receber ao menos `AVK_CLR`/`AVK_SELECT` por rota de teclado;
- rede do guest fica desligada sem escolha explícita;
- todos os exports base existem;
- ausência de save state é declarada corretamente, sem estado parcial;
- build e carregamento AArch64 são validados antes de release.
