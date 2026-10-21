# 25 — Conquistas: o RetroAchievements pelo rcheevos

O Zeebx fala com o [RetroAchievements](https://retroachievements.org) (RA) pelo
[rcheevos](https://github.com/RetroAchievements/rcheevos), a biblioteca em C que o próprio RA
mantém e que o RetroArch, o DuckStation e o PCSX2 também usam. O plano inteiro, com as fases e o
que depende dos mantenedores do RA, está na issue de rastreamento; este documento diz o que já
está no código e por quê.

| Peça | Onde | Estado |
|---|---|---|
| O rcheevos | `rcheevos-sys/` (crate) e `rcheevos-sys/rcheevos/` (submódulo do fork) | compila no Linux e no Android; Windows e macOS só pelo CI |
| O cliente | `src/conquistas/`, feature `conquistas` | login, jogo, quadros, hardcore, progresso, HTTP |
| A interface | `frontends/qt-standalone`, feature `conquistas` | categoria, menus, avisos, lista, detalhes |
| O mapa de memória | `Mapa::provisorio` | **em aberto**, a combinar com o RA |
| O hash | `rc_hash` do fork | MD5 do `.mod`; o PR ao rcheevos ainda não foi aberto |

**A feature vem desligada, nas duas crates.** Sem o Zeebo cadastrado no servidor do RA, todo jogo
volta como desconhecido, e uma aba de conquistas que não faz nada seria pior que nenhuma. Ligar a
feature na release depende de três coisas: o console aceito pelo RA, o rcheevos oficial no lugar
do fork, e o aviso MIT do rcheevos no `THIRD-PARTY-NOTICES.txt` (a crate é privada, e o
`cargo about` a ignora).

```bash
git submodule update --init rcheevos-sys/rcheevos
cargo build --release -p zeebx-standalone-qt --features conquistas
cargo test  --release -p zeebx -p rcheevos-sys --features conquistas
```

O core Libretro **não** liga o rcheevos: o RetroArch já o integra e lê a memória do core sozinho.
O que o core vai precisar é expor a memória, e isso espera o mapa (ver [O mapa](#o-mapa-de-memória)).

## O que o RA exige

A lista inteira está em
[Hardcore Compliance Requirements](https://docs.retroachievements.org/general/hardcore-compliance-requirements.html).
O que pesou no desenho:

- no **hardcore**, nada de carregar estado, rewind, câmera lenta, frame advance, trapaça,
  depurador ou editor de memória;
- entrar no hardcore no meio de uma partida exige **reiniciar o jogo**; sair é livre;
- o modo aparece na tela quando o jogo começa, e a lista de conquistas tem de estar ao alcance;
- um user agent único: `Zeebx/0.4.2 (Linux) rcheevos/…`;
- desbloqueio hardcore só para emuladores públicos há pelo menos **6 meses**.

O Zeebx tem pre-releases públicas desde 14/09/2026 (v0.1.0). Se o RA as contar, o prazo vence por
volta de 14/03/2027. Isso ainda precisa ser confirmado com eles.

## O rcheevos

**Um submódulo do fork, e não o oficial.** O `RC_CONSOLE_ZEEBO` (70) existe no oficial desde a
v10.3.0, mas só como número e nome: não há mapa de memória nem hash. O fork traz o hash, e é ele
que vira o PR para o upstream. O endereço no `.gitmodules` é HTTPS para que qualquer pessoa e o CI
clonem sem chave. Quem mexe no fork aponta `RCHEEVOS_DIR` para a própria cópia e compila o Zeebx
contra ela, sem empurrar nada.

**Compilado pelo `cc`, e não pelo Makefile dele.** O `Makefile.common` do rcheevos só conhece x86 e
x64, e o Zeebx compila também para Android, iOS e Switch. Ficam de fora o hash de disco, o de ROM
cifrada e o de zip: são de consoles que o Zeebx não emula.

**Os bindings são escritos à mão.** O `bindgen` pediria o libclang em cada alvo, e hoje só o
`coreaudio-sys` do macOS o puxa. O risco de uma cópia à mão é que um struct errado não dá erro de
compilação: lê o campo vizinho e segue. Por isso o `rcheevos-sys/src/tamanhos.c` compila junto e
devolve o `sizeof`, o `offsetof` e o valor de cada struct e constante copiados, e um teste compara
com o Rust. Ele pegou um erro na primeira escrita: o `RC_NO_GAME_LOADED` é -29, e estava -30. O
`time_t` no meio do `rc_client_achievement_t` é o motivo principal da conferência: ele muda de
tamanho entre alvos, e tudo o que vem depois dele muda de lugar.

## O cliente

O `Conquistas` embrulha o `rc_client` e vive **acima** da sessão: a conta continua conectada de um
jogo para o outro. A sessão o recebe emprestado (`Session::liga_conquistas`) e o devolve ao
fechar. O emulador entrega três coisas a ele.

**A memória, só dentro do quadro.** O `rc_client` é configurado para não ler memória fora do
`do_frame` e do `idle` (`rc_client_set_allow_background_memory_reads`). A consequência, medida no
primeiro teste que falhou: **o `JogoCarregado` só chega no primeiro quadro depois da resposta do
servidor**, e não quando ela chega. É ali que o rcheevos confere se cada endereço se lê, e sem
memória ele desligaria todas as conquistas. Um hash desconhecido, que não tem o que conferir, chega
na hora.

**O servidor, por uma fila.** Cada pedido do rcheevos vai a um `Servidor` com uma `Entrega`. O de
verdade (`http.rs`) abre uma thread por pedido com o `ureq`; o dos testes responde na hora, com o
JSON do servidor do RA copiado dos testes do próprio rcheevos. A resposta pode chegar de qualquer
thread, mas só é passada ao rcheevos na thread do cliente, no quadro ou no `ocioso` seguinte. Uma
entrega descartada sem resposta responde sozinha como falha: o rcheevos só libera a memória do
pedido no retorno, e um login pendurado para sempre seria pior que um erro.

**O mapa.** Ver [O mapa de memória](#o-mapa-de-memória).

O estado que os retornos do C alcançam mora numa caixa à parte, acessada só por ponteiro cru dos
dois lados. Uma chamada ao rcheevos pode voltar a um retorno nosso, e um `&mut` vivo de um lado com
o outro lado mexendo no mesmo lugar seria comportamento indefinido.

### O quadro

As conquistas são avaliadas a cada **quadro de 60 Hz do relógio virtual** (`Session::acompanha_as_conquistas`),
o mesmo retraço que o `eglSwapBuffers` do console espera. Contar desenhos faria o frameskip, o
limite de 30 fps e o fast-forward mudarem quantas vezes uma conquista é avaliada; contar o relógio
dá a mesma contagem em qualquer máquina e em qualquer velocidade, e serve também aos jogos 2D, que
nem passam pelo EGL. Um teste confere que, sem o relógio andar, nenhum quadro é avaliado, por mais
voltas que o laço dê. Depois de um save state a contagem reancora: o relógio foi para onde o estado
mandou.

### O hash

**O MD5 do `.mod` principal**, o mesmo que o `find_module` escolhe. É o que os endereços das
conquistas pressupõem: um `.mod` diferente pode mover tudo. É idêntico com o jogo solto, em `.zip`
ou em `.7z`, e não muda com os saves que o jogo grava dentro da própria pasta. O `.mif` e os
módulos de extensão ficam de fora.

**A conta é do `rc_hash`, e não nossa.** O fork põe o Zeebo na lista de consoles cujo hash é o
arquivo inteiro, e a sessão chama o `rc_hash_generate_from_buffer` sobre os bytes que acabou de ler
para o loader. Assim o hash de referência mora num lugar só. No Double Dragon ele bate com o
`md5sum` do arquivo.

**A extensão `.mod` não identifica o Zeebo** no `rc_hash`: ela também é a dos módulos de música, e a
decisão é dos mantenedores.

**Deixar as extensões de fora quase não custa.** Medido em 2026-10-14 com o `library::extensoes` na
biblioteca inteira: **1 dos 74 jogos** traz módulo de extensão — o Action Hero 3D, com o
`imicro3d.mod` (90.068 bytes, ClassID `0x010292c3`), que pelo nome é o motor 3D IMICRO3D, e não o
jogo.

## O hardcore

| Regra | Como |
|---|---|
| Carregar estado | o `Session::restaura_estado` recusa com `save_state::Erro::Hardcore`; a janela pergunta e chama o `Session::sai_do_hardcore` antes |
| Rewind | o `Session::pode_marcar_ponto` recusa, e o anel nem grava; o atalho devolve `Leitura::Hardcore` uma vez por aperto, e a janela avisa na tela |
| Ligar com o jogo aberto | o rcheevos pede o reinício (`Evento::Reiniciar`); o Qt abre o mesmo arquivo de novo |
| Sair | livre, sem reinício |
| Fast-forward, turbo, frameskip, limite de 30 fps | permitidos: o limite de 30 fps não desacelera a lógica |
| A ponte de rede (`src/ponte.rs`) | permitida: só deposita a resposta do servidor na memória do jogo, e vem desligada |

**As travas moram na sessão, e não nas telas.** Quando elas foram escritas, o Qt ainda não tinha
save state, e uma tela que viesse depois esqueceria a regra. Foi o que aconteceu: os dez slots do
desktop (`docs/implementacao/25-save-state-no-desktop.md`) chegaram depois, e o carregar deles já
nasceu recusado no hardcore, porque passa pelo `Session::restaura_estado`. A tela só acrescenta a
educação: o menu Sistema desabilita o Carregar estado e o Desfazer carregar, e o atalho responde
"Indisponível no modo hardcore" em vez do erro da sessão, sem gravar o estado de agora para um
desfazer que não pode vir. Salvar continua livre, como o RA permite.

**O hardcore vale quando há o que proteger** (`Conquistas::protege_o_jogo`): ligado, e com um jogo
que tem conquistas **ou que ainda está a caminho**. Entre o pedido e a resposta do servidor o set
ainda não chegou; carregar um estado nesse intervalo e ver o set chegar depois seria entrar no
hardcore com a partida mexida. Um jogo que o RA não conhece solta as regras.

O hardcore é global, e não entra nas configurações por jogo. O padrão é casual: o RA só aceita o
hardcore de emuladores aprovados.

## O progresso no save state

O save state leva o progresso das conquistas — os contadores de acerto e os placares em
andamento — numa seção `conquistas`, que a sessão acrescenta pelo `Machine::grava_estado_com`. Sem
ela, voltar a um estado deixaria uma conquista "três quadros seguidos" com a contagem de depois do
estado. O restaurar da máquina procura cada seção pelo nome, então um estado com a seção carrega
num Zeebx sem conquistas, e um estado antigo, sem ela, carrega no novo e zera os contadores, como o
rcheevos manda. Os dois sentidos têm teste.

## A interface

| Onde | O quê |
|---|---|
| Configurações → Conquistas (categoria com o troféu, e o item no menu Configurações) | login, conta, sair; o hardcore, que pergunta antes de reiniciar um jogo aberto; os dois avisos. A explicação de cada um vai para o rodapé, como nas outras categorias |
| Ao abrir o jogo | o título, "x de y conquistas" e o modo; ou "Sem conquistas para este jogo" |
| Durante o jogo | os avisos no canto de baixo à esquerda, como no RALibretro, numa fila de quatro segundos cada: desbloqueio com ícone, placares, conexão; o contador dos placares no canto de baixo à direita |
| F6 (trocável, tecla ou botão), ou Sistema → Lista de conquistas | a lista por cima do jogo, que fica pausado; o F6 ou o Esc fecham. O P, o R e o menu fecham a lista antes de fazer o que fazem |
| Detalhes da biblioteca | "Conquistas: x de y", da última vez em que o jogo foi aberto com a conta |

**A senha não é guardada.** O login devolve um token, e é ele que vai ao `settings.json`, como no
RetroArch; a senha é apagada até do campo assim que vai ao servidor. Na abertura, o Zeebx entra
sozinho com o token; um token recusado é esquecido, para não falhar do mesmo jeito em toda
abertura.

**Um jogo aberto antes de o login terminar ganha o cliente depois.** O `zeebx-qt <jogo>` abre o jogo
antes do primeiro login com o token, e quem abre um jogo logo depois de iniciar o Zeebx também
chega antes da resposta. A primeira versão deixava essa sessão sem conquistas e, pior, parava de
entregar as respostas ao cliente, porque com um jogo aberto quem anda com os pedidos é o quadro da
sessão — e a sessão não tinha cliente. Achado com o servidor simulado, na janela de verdade. Agora,
enquanto nenhuma sessão tem o cliente, o relógio da biblioteca anda com os pedidos; quando o login
termina, a sessão aberta o ganha. No hardcore, o jogo recomeça em vez disso: ele rodou até ali sem
as travas, e o RA exige o hardcore desde o começo da partida.

**Os ícones são baixados uma vez** (`<config>/conquistas/icones`). Ao reconhecer o jogo, o Zeebx já
pede os de todas as conquistas, para o aviso de desbloqueio sair com o ícone.

**O objeto `Conquistas` do QML existe com ou sem a feature.** O cxx-qt gera a ponte do arquivo, e
uma ponte que só existisse com a feature pediria um `build.rs` e um QML para cada caso. Sem a
feature, `Conquistas.disponivel()` responde falso, e a categoria, os dois itens de menu, a linha do
atalho e a lista somem. O F6
continua entre as teclas reservadas e não chega ao jogo; nenhum mapeamento padrão o usa.

Os indicadores de desafio e de progresso (os ícones pequenos de "quase lá") ficaram para depois: a
lista e os avisos cobrem o que o RA exige do *Measured*.

## A fila de desbloqueios sem rede

O rcheevos guarda **só na memória** o desbloqueio que não chegou ao servidor: ele tenta de novo
enquanto o Zeebx está aberto, e fechar o emulador sem rede o perderia. O RA exige que ele fique
guardado e seja enviado quando a rede voltar. A fila (`src/conquistas/pendentes.rs`,
`<config>/conquistas/pendentes.json`) anota cada desbloqueio e cada envio de placar **antes** de ele
sair, e só o tira com uma resposta definitiva — um 2xx, ou um 4xx que não seja 429. O que sobra de
uma execução é mandado de novo na seguinte, logo depois do login, e a resposta só tira o pedido da
fila: ela não volta ao rcheevos, que não fez esse pedido.

**Guarda-se o que o pedido significa, e não o pedido.** A primeira versão guardava a URL e o corpo,
e passou nos testes com servidor simulado. Na janela de verdade, com o servidor simulado recusando
os desbloqueios com 503, ela falhou de dois jeitos:

- **12 tentativas de 2 conquistas viraram 10 entradas.** A cada tentativa nova o rcheevos acrescenta
  `o=<segundos desde o desbloqueio>`, que muda a assinatura `v`; o corpo nunca se repete.
- **A execução seguinte mandou para o servidor da anterior.** A URL guardava o host.

Agora a entrada é a conquista (ou o placar e o valor), o modo, o hash do jogo, a conta e o
**instante** do desbloqueio. O pedido é remontado pelo próprio rcheevos
(`rc_api_init_award_achievement_request_hosted`) na hora do envio, com o servidor e o token de
então e o `o` certo: na conferência, dois desbloqueios guardados sem rede foram reenviados a outro
servidor com `o=31` e `o=27`, e a fila esvaziou. **O token não vai para o arquivo**, e uma entrada
de outra conta espera o login dela.

Para montar o pedido, o `rc_api_request_t` passou a ser copiado inteiro nos bindings. Ele guarda as
strings num buffer que aponta para dentro dele mesmo, e por isso não pode mudar de lugar entre o
`init` e o `destroy`: é montado e lido na mesma função.

## O mapa de memória

**É a decisão que falta, e é um contrato permanente.** Uma conquista publicada guarda endereços; um
mapa que mude depois quebra os sets que já existem. O mapa do Zeebx é do emulador, e não do
aparelho, porque a emulação é de alto nível (ver [02-cpu-e-memoria.md](02-cpu-e-memoria.md)). Por
isso a tradução "endereço do RA → endereço do guest" mora num lugar só, o `Mapa`, e o
`Mapa::provisorio` é a identidade sobre os 4 GiB: o que o guest não mapeia não se lê, e o
`rc_client` desliga a conquista. Sem mapa no `consoleinfo.c`, essa leitura é a única validação que
ele faz.

**A medida que pesa na conversa.** A primeira proposta era usar a região do módulo — a imagem, a
`.data` e a `.bss` — como âncora estável, porque ela fica sempre no mesmo endereço, e alcançar o
heap só por ponteiro. Medido com o teste de ponta a ponta, depois de 3 s de jogo — a primeira vez em
2026-10-13, e de novo em 2026-10-17, depois de os blocos do emulador saírem do heap (ver abaixo):

| Jogo | Região do módulo | Algum byte dela mudou em 0,5 s? | Algum byte dela muda a cada 100 ms? | Primeiro byte do heap que muda a cada 100 ms, antes | e depois da região interna |
|---|---|---|---|---|---|
| Double Dragon | 1.507.328 bytes | não | não | `0x1000005c` | `0x1000005c` |
| Pac-Mania | 1.196.032 bytes | não | não | `0x100014c8` | `0x100014c8` |
| Peggle | 1.318.912 bytes | não | não | `0x100000a0` | **`0x10000088`** |
| Crash Bandicoot Nitro Kart 3D | 1.695.744 bytes | não | não | `0x10000090` | `0x10000090` |
| Galaxy on Fire | 1.519.616 bytes | não medido | não | `0x1000004c` | `0x1000004c` |
| Quake | 1.486.848 bytes | não medido | não | nenhum | nenhum |

**Nos quatro jogos medidos de perto, nada muda no módulo; o estado vivo está no heap, já nos
primeiros bytes.** A hipótese, não conferida no código dos jogos, é o costume do BREW de guardar o
estado do jogo na struct do applet, que o `AEEApplet_New` aloca no heap logo na abertura. Se for
isso, a âncora estável não é o módulo, e sim o começo do heap.

**O heap passou a ser só do jogo.** O que o emulador aloca para si no espaço do guest — pilhas de
`IThread`, o buffer do `IPeek`, o `AEEImageInfo`, strings internas — morava no heap, intercalado com
o que o jogo pede, e um bloco nosso a mais empurrava tudo o que vinha depois. Agora sai de uma região
própria, `0x34000000` (16 MB), pelo `aloca_interno` (ver [02-cpu-e-memoria.md](02-cpu-e-memoria.md)).
A coluna "depois" mostra o efeito: no Peggle o estado andou 24 bytes, de `0x100000a0` para
`0x10000088`, porque um bloco do emulador alocado antes dele saiu do caminho; nos outros cinco nada
mudou, porque nenhum bloco nosso vinha antes do estado. Daqui em diante o endereço de cada estrutura
depende só do que o jogo pede e da regra do nosso alocador — que precisa ficar a mesma, e é isso que
se propõe ao RA. Na varredura das 74 ROMs com 6 s de jogo, nenhuma encheu os 16 MB da região
interna. Os PRs do rcheevos que servem de referência para
a conversa: o [#446](https://github.com/RetroAchievements/rcheevos/pull/446) (3DS, endereços
virtuais um para um e leitura que falha marcando a faixa como vazia) e o
[#472](https://github.com/RetroAchievements/rcheevos/pull/472) (um console com mais de 16 MB e uma
faixa vazia maior que 64 KB exige que o core Libretro declare o mapa por
`RETRO_ENVIRONMENT_SET_MEMORY_MAPS`; o heap de 64 MB do Zeebx cai nessa regra).

## Os testes

| Teste | O que prova |
|---|---|
| `rcheevos-sys`: `os_structs_batem_com_o_c`, `as_constantes_batem_com_o_c` | os bindings copiados à mão batem com o C no alvo |
| `conquistas::testes` | login, token, jogo desconhecido, desbloqueio casual, endereço que não se lê, hardcore, progresso, lista e resumo, com o servidor simulado |
| `conquistas::http::testes` | o login sai por HTTP de verdade para um servidor em `127.0.0.1`: POST, corpo, `Content-Type` e user agent |
| `conquistas::pendentes::testes` e `o_desbloqueio_sem_rede_vai_na_execucao_seguinte` | a fila: só desbloqueio e placar, as tentativas com `o` e `v` diferentes são uma entrada só, sem token no arquivo, e o reenvio vai ao servidor de agora com há quanto tempo foi |
| `session::tests::as_conquistas_contam_quadros_pelo_relogio_virtual` | sem o relógio andar, nenhum quadro |
| `session::tests::o_hardcore_recusa_…`, `o_rewind_no_hardcore_…`, `o_save_state_leva_…` | as travas e o progresso, na sessão |
| `session::tests::uma_conquista_dispara_num_jogo_de_verdade` | com `ZEEBX_TESTE_ROM`, uma conquista sobre a memória que o Dynarmic executa dispara |

O teste com jogo de verdade não sabe o que cada endereço significa — esse é o trabalho de quem vai
fazer os sets. Ele acha sozinho um byte do módulo, ou do heap, que muda a cada 100 ms, e define a
conquista "este byte mudou desde o quadro anterior, dez vezes" (`0xH…!=d0xH….10.`). **Dez acertos, e
não um:** o rcheevos só arma uma conquista depois de vê-la falsa num quadro, e no Crash o byte
achado é a parte baixa de um contador de milissegundos, que muda em todo quadro. Passa no Double
Dragon, no Pac-Mania, no Peggle, no Crash e no Galaxy on Fire; no Quake não há byte que mude a cada
100 ms para usar.

```bash
ZEEBX_TESTE_ROM="…/peggle.mod" cargo test --release -p zeebx --features conquistas \
    --lib -- uma_conquista_dispara --nocapture
```

**O que não tem teste automático:** a interface Qt, validada à mão — o login com uma conta real
funciona. Com o servidor do RA de verdade, todo jogo do Zeebo é desconhecido, e por isso o
desbloqueio, os placares, a lista com itens e o hardcore na janela só se veem com um servidor
simulado.

### O servidor simulado

O `ferramentas/servidor_ra_simulado.py` responde como o servidor do RA para **um** jogo, só com a
biblioteca padrão do Python e só em `127.0.0.1`. O Zeebx fala com ele pela variável
`ZEEBX_RA_HOST`, que troca o servidor de qualquer `Conquistas::novo` e deixa um aviso no registro:

```bash
python3 ferramentas/servidor_ra_simulado.py --jogo "…/Peggle (Brazil) (Es,Pt)/mod/278962/peggle.mod"
ZEEBX_RA_HOST=http://127.0.0.1:8090 ./target/release/zeebx-qt
```

Qualquer usuário e senha entram. O conjunto tem duas conquistas sobre um byte que muda sozinho (o
`0x10000088` do Peggle, da tabela acima; outro jogo pede `--endereco`), uma com progresso medido,
uma sobre um endereço que o Zeebx não lê — para cair em "sem suporte" — e um placar. Os ícones são
quadrados de uma cor, dourado e cinza. `--sem-rede-para awardachievement` responde 503 aos
desbloqueios, que é como ver a fila de pendentes funcionar.

Com ele, na janela de verdade e fora de tela, o Peggle fez o caminho inteiro: login pelo token, o
jogo pelo hash, a sessão, três desbloqueios, o rich presence, os nove ícones baixados e o resumo
"3 de 4" gravado. Foi assim que apareceram o jogo aberto antes do login e os dois defeitos da
primeira fila, que os testes com servidor simulado dentro do processo não pegavam.

## O que falta

- Fechar o mapa de memória com os mantenedores do RA.
- O PR ao rcheevos: o mapa no `consoleinfo.c` e o hash no `rc_hash`.
- O console cadastrado e ativo no servidor do RA.
- Trocar o fork pelo rcheevos oficial e ligar a feature na release, com o aviso de licença.
- O RAIntegration no Windows, para quem cria os sets.
- O `RETRO_ENVIRONMENT_SET_MEMORY_MAPS` no core Libretro.
- Android e iOS: a lógica está no núcleo, falta a interface.
