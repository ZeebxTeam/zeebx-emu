# Notas da sessão — 21/09/2026

O que foi medido, o que mudou e o que ficou por fazer. Escrito para quem vai continuar: cada
achado traz o sintoma **e** a evidência, e nenhum deles é dedução.

## O "Memory is insufficient" do Double Dragon

**Causa medida:** o jogo abre `./udata/ddz.sav` com `OFM_CREATE` **sem chamar `MkDir` antes**, e o
diretório `udata/` não existia. Um refactor desta sessão (o commit `748525c`, que extraiu o bloco de
arquivos para `machine/file.rs`) perdeu, na mudança, o `create_dir_all` do diretório-pai que
existia no ramo de criação. A abertura falhava com `No such file or directory`, e o jogo — que não
distingue "não consegui abrir" de "acabou a memória" — mostrava a tela de falta de memória.

Isso explica o que a equipe viu: no `zeebx padrão` (upstream) o `create_dir_all` está lá, e por isso
o Kaio nunca teve o problema; o Tênis do João funcionava porque não cria arquivo em pasta nova no
arranque.

**Como se mediu:** o relatório da varredura passou a registrar as chamadas do `IFileMgr` com
caminho, argumentos e retorno, e o caminho do host com o erro do sistema quando a abertura falha.
A linha que resolveu:

```text
OpenFile "./udata/ddz.sav"  (0x5e078 0x4 0xf0003008) -> 0
open falhou: <cache>/…/mod/274754/udata/ddz.sav (No such file or directory)
```

Depois do conserto: `OpenFile "sound.ggz" -> 805307728`, o `.sav` é criado, e a tela desenhada passa
a ser `CARREGANDO`.

## Três armadilhas de CI, todas medidas

1. **Push cancela push.** Quatro execuções seguidas do workflow `ci` terminaram `cancelled`, porque
   cada push novo derrubava a matriz de seis alvos que estava no meio — e o CI do standalone
   **nunca** tinha rodado até o fim. Corrigido: `cancel-in-progress` só em PR.
2. **`continue-on-error: ${{ matrix.experimental }}` faz o GitHub rejeitar o arquivo inteiro.** O
   sintoma é um run **sem nenhum trabalho** (`gh api .../runs/<id>/jobs` devolve `total_count: 0`).
   Nem `actionlint` nem um leitor de YAML acusam. Regra: run vazio é o arquivo, não a máquina.
3. **Os testes nunca compilavam o alvo do binário**: o `main.rs` importava `zeebx::varredura` sob
   `#[cfg(test)]`, e nesse alvo a biblioteca é compilada **sem** o `cfg`. Rodar `cargo test --lib`
   escondia isso.

Mais dois, de plataforma:

- **macOS**: dois testes de VFS comparavam a caixa exata do caminho, e o APFS **não distingue
  caixa** — não há duas grafias para comparar. O mesmo tipo de defeito apareceu no Windows x86_64,
  num teste que procurava `/zeebx/` no texto (o Windows usa `\`).
- **Windows ARM64**: o `unicorn` (QEMU em C) **não compila** ali — o QEMU monta
  `setjmp-wrapper-win32.asm` com o `ml` do MSVC, que só existe para x86 e x64. O `unicorn-engine`
  2.1.5 é a última versão publicada. Solução: dependência condicional de alvo, e o `dynarmic` passa
  a ser o backend padrão naquele alvo, com os ganchos de depuração do unicorn implementados como
  recusa explícita.

## Dois jogos estavam mudos, e o que os calava

O relatório lista os sons recusados. Fazer a recusa **dizer o formato e os primeiros bytes** custou
uma linha e resolveu os dois casos:

```text
Turma da Mônica   recusado (formato desconhecido, 86 sons, 4f 67 67 53 …)   ← "OggS"
Peggle            recusado (audio/mpeg, 7 sons, 49 44 33 03 00 00 00 00 …)   ← "ID3"
```

| Jogo | Antes | Depois |
|---|---|---|
| Peggle | 7 recusas, pico **0,000** | 0 recusas, pico **0,356** |
| Turma da Mônica | 86 recusas, pico **0,000** | 0 recusas, pico **0,586** |

- **Ogg/Vorbis**: o `symphonia` já sabia decodificar; faltava ligar a feature.
- **MP3 com etiqueta ID3 que mente**: a etiqueta declara **zero byte** de tamanho e tem trinta mil de
  conteúdo. Quem pula pelo campo declarado procura o quadro no lugar errado e recusa o arquivo
  inteiro. Agora a busca é pela **sincronia do quadro**.

**Lição que vale para o resto do motor:** uma recusa que não diz *o que* chegou custa uma
investigação inteira dentro do jogo.

## Desempenho, medido

- **Rolima**: 6,8 milhões de chamadas de API em seis segundos, num laço de espera que o atalho de
  ocioso não reconhecia (relógio, `memset`, consulta de controle). Depois de reconhecê-lo:
  **79% → 284%**, instruções de 2,46 bilhões para 886 milhões. 51 jogos ficaram mais rápidos, e
  **nenhum estado mudou** em quatro varreduras do corpus.
- **Onde o tempo vai** (`ZEEBX_ROM_PERFIL=1`): no Crash Nitro Kart, `eglSwapBuffers` é **89,8%** do
  tempo de API — é o rasterizador software desenhando a cena. Ainda assim o jogo roda a 1039%.
- **Lacuna de GL**: a seção "GL atendido sem fazer nada" apontava **uma** chamada nas 62 ROMs, o
  `glPixelStorei` (alinhamento de linha na textura), em seis jogos. Implementado; depois, **zero**
  jogos ignoram qualquer chamada de GL.

## O que a varredura pegou hoje, e o que foi corrigido

A varredura com linha de base commitada deixou de ser relatório e passou a ser teste. No mesmo dia
em que passou a cobrar, ela encontrou quatro defeitos — e três foram corrigidos:

| Defeito | Correção | Resultado medido |
|---|---|---|
| `glPixelStorei` ignorado (seis jogos) | alinhamento de linha na textura | zero jogos ignoram GL |
| Dois jogos mudos | Ogg/Vorbis e o quadro MPEG pela sincronia | pico 0,000 → 0,356 e 0,586 |
| `AEECLSID_JPEGDecoderBREW` recusado | uma linha na fábrica | o Zuma avança um portão |
| O decodificador só tentava PNG | despacho pela assinatura | **o Zuma roda**, 18.268 cores |

Também entrou uma tela preta que passava como "roda" (o Prey Evil), e o passo de áudio que sumia de
uma vez virou descida de 1,5 ms — sem mudar a medição dos 18 saltos da Peteca, que se revelaram os
**ataques** dos efeitos, e não estalos.

## O estado do render em hardware

Não falta código. O que existe, medido:

- o rasterizador da placa desenha **o mesmo quadro** que o de software (idêntico em Double Dragon e
  Crash; 2,6% de arredondamento no Peteca);
- o motor sabe desenhar num framebuffer **de fora** (teste que lê os pixels do framebuffer alheio);
- as features `gl` e `gpu` estão separadas, e o core usa só a primeira: **0 dependências de host**,
  medido em CI nos seis alvos;
- o core pede o contexto, monta o `glow::Context`, desenha no FBO do frontend, e **volta ao software**
  se qualquer passo falhar — inclusive num pânico, que derrubaria o RetroArch;
- os deslocamentos do `retro_hw_render_callback` batem com o `libretro.h`, e há teste que os cobra.

### Verificado no RetroArch de verdade, e dois defeitos que só apareciam ali

Rodar o core no RetroArch 1.20 desta máquina (Wayland/EGL, AMD renoir, Mesa 25.0.7) encontrou dois
defeitos que nenhum teste headless pegava, e os dois só se revelavam com o frontend apresentando o
quadro:

1. **O perfil do contexto estava errado.** O core pedia `RETRO_HW_CONTEXT_OPENGL` (compatibilidade)
   com versão 3.3. Um pedido de versão 3.2+ **sem máscara de perfil** faz o EGL devolver um contexto
   *core*, e o driver `gl` do RetroArch é de compatibilidade — ele quebrava antes de rodar um quadro:

   ```text
   [INFO] [GL]: Version: 4.6 (Core Profile) Mesa 25.0.7
   [ERROR] [GL]: GL: Invalid enum.
   [ERROR] [Video]: Cannot open video driver.. Exiting..
   ```

   O emulador standalone pede GL 3.3 e o `glutin` entrega **core**, e é nele que o motor foi medido.
   Agora o core pede `RETRO_HW_CONTEXT_OPENGL_CORE` e o RetroArch usa o caminho `glcore`.

2. **O alvo do desenho era religado no nosso framebuffer a cada quadro.** A escolha do alvo estava
   escrita em dois lugares de `destino()`, e os dois discordavam: o caminho que **cria** o destino
   respeitava o framebuffer do frontend, e o caminho curto — destino já pronto, mesmos parâmetros,
   que é o que roda em todo quadro depois do primeiro — religava o nosso. Resultado: desenhávamos no
   nosso framebuffer, o RetroArch apresentava o dele e a tela ficava preta enquanto a placa
   trabalhava o mesmo tanto. Medido lendo 4×4 pixels do framebuffer do frontend:

   ```text
   antes:  [0, 0, 0, 254] × 4     ← preto, e a captura com 1 cor
   depois: [254, 254, 254, 0] × 4 ← desenhado, e a captura com 1282 cores
   ```

   A escolha passou a viver em `alvo_do_desenho()`, para as duas ramificações não poderem discordar
   de novo, e o teste do framebuffer de fora ganhou uma **cerca**: pinta o framebuffer de verde no
   meio do percurso e exige que os quadros seguintes o sobrescrevam. Sem ela o teste passava com o
   defeito, porque só provava que *algum* quadro tinha ido para lá. Com a cerca, o defeito
   reintroduzido de propósito deixa 100% dos pixels verdes e o teste falha.

### Medindo cor no RetroArch: cuidado com o shader

A contagem de cores de uma **captura do RetroArch** mede o quadro **depois** do shader — nesta
máquina há um filtro de CRT ativo. Ela não serve para julgar o que o core desenhou. O que vale é
ler os pixels do framebuffer que o core recebeu (`glReadPixels` no FBO do frontend), como na medição
acima: ali ainda não houve shader.

### O arredondamento entre os dois rasterizadores

O teste que compara os dois rasterizadores comparava em passos de canal RGB565 com tolerância de
60% dos pixels. Rodando com o Crash, 98,33% dos pixels diferem — mas por **1 ou 2 passos**, média
1,93, pior 2, e **0,00% acima de dois passos**: o rasterizador de software trunca (`>> 3`) e o
`glReadPixels` em `UNSIGNED_SHORT_5_6_5` arredonda. A conta passou a separar as duas coisas: uma
imagem diferente erra por muito mais que dois passos. Alinhar os dois conversores é decisão que
precisa do console na mão, e fica declarada aqui.

## O que falta

**Reescrito em 22/09/2026 com o que está medido hoje.** O que estava aqui antes dizia que a
varredura não conseguia exercitar a roda e que os save states não existiam — as duas coisas caíram,
e a segunda caiu por medição, não por opinião.

- **Item 5** (render em hardware): o rasterizador da placa está verificado contra o de software
  (idêntico em Double Dragon e Crash; 2,6% de arredondamento no Peteca) e o encanamento com o
  frontend existe. O que falta é o **teste físico no aparelho** — bloqueio de hardware, declarado.
- **Item 6** (save states): **feito, e provado pela ABI**. `retro_serialize_size` devolve o tamanho
  real do arquivo da sessão, e o teste `o_save_state_atravessa_a_abi` grava, suja o estado, carrega
  e **grava de novo byte a byte igual** — que é o que um save state promete. O que continua
  proibido é o estado **parcial**: o portão só abre quando a sessão inteira pode ser descrita.
- **Item 8** (ciclo da Z-Wheel): **o lançamento está provado no harness headless**, roteirizado,
  com `Session::take_launch_request` como desfecho — `abertura pedida: 0x0108e356`, com a grade da
  biblioteca desenhada antes. O caminho do **core** ganhou duas peças medidas (a tradução do
  controle em teclas do console, sem a qual a roda ignorava o RetroPad, e o catálogo da biblioteca
  local), mas **ainda não pede a abertura**: a diferença está em como o core conduz a sessão, não
  na tecla. A validação **no aparelho** continua bloqueada por hardware.
- **Item 9** (capas e No-Intro): local pronto e medido — playlist com 62 entradas e **nenhuma sem
  arquivo**, 58 capas no formato e no nome que o repositório de thumbnails do Libretro procura
  (`Mobile - Zeebo`), DAT No-Intro de 57 jogos. **Publicar depende de conta**, e é decisão do
  Rafael; nada foi publicado e nenhum repositório externo foi criado.

**Bloqueado por hardware, sem contorno:** item 5 (teste físico) e a validação no aparelho do item 8.
O core no cartão do muOS, que estava adiado neste ponto do histórico, foi instalado depois no SD1;
a validação física continua pendente.

## O que o teste headless já responde sobre a Z-Wheel

Eu tinha escrito que a roda **não responde** sem frontend. Isso estava errado, e o erro era do
teste: ele parava no primeiro botão que não mudava a imagem e concluía "nada responde". Medindo
botão por botão, com o teste que conta imagens distintas:

```text
botao 3 (START):  1 imagem
botao 8:          1 imagem
botao 0:          1 imagem
botao 2 (SELECT): 2 imagens   ← a roda reagiu
manche x e y:     1 imagem    (não moveu)
```

**A roda responde — e isso foi medido com tempo suficiente.** Com 1200 quadros de espera (vinte
segundos virtuais) antes de mandar entrada:

```text
botao 3 (START):   1 imagem
botao 8:          12 imagens   ← reagiu
manche x=+32767:  26 imagens   ← reagiu
```

Ou seja: **a roda fica interativa por volta dos vinte segundos**, e o caminho de entrada dela
funciona pelo core. As duas medições anteriores que diziam "não responde" estavam medindo
**carregamento** — a mesma armadilha dos títulos F.C., pela terceira vez no dia. O teste agora
espera antes de mandar entrada, e diz qual entrada mudou a tela.

**O que ainda não saiu é a abertura de um jogo.** Com a roda interativa e navegando, o teste tenta
manche (quatro direções) com cada botão de face e o Start, e espera 180 quadros depois do
confirmar — a transição da roda é animada. O pedido não veio. O que já está descartado, por
medição: a entrada (chega), o tempo de carregamento (vinte segundos bastam), o tempo depois do
botão (180 quadros), e a árvore de aparelho vazia (o teste foi rodado com `ZEEBX_CORE_SISTEMA`
apontando para a pasta real, com os 63 jogos instalados). O que sobra é a **sequência** — qual
gesto a roda espera para confirmar.

O teste agora cobre, e nada disso abriu jogo: manche nas quatro direções; cada um dos quatro botões
de face **e** o Start; o manche empurrado com o botão apertado sem soltar **e** solto antes de
apertar; 180 quadros de espera depois do confirmar (a transição é animada); e a árvore de aparelho
verdadeira, com os 63 jogos instalados. Tudo com o `ULTIMA_ABERTURA` como instrumento — ele diz se
o pedido saiu, e não saiu.

**E os valores que ela lê estão certos, conferido no SDK.** Cheguei a desconfiar de um defeito no
`GetPositionState` — o primeiro campo é escrito como zero, e eu li ali o estado dos botões. O
`AEEIHIDDevice.h` diz o contrário: o primeiro campo é `bRelativeAxes`, e zero ali significa **eixo
absoluto**, que é o certo. Os quatro eixos estão nas palavras 1, 2, 3 e 6, exatamente como o engine
os escreve (`AXIS_SLOTS`). Quem acertou foi o código; quem errou foi a leitura rápida.

## Onde o gesto da Z-Wheel para, medido

Depois de o roteiro de teclas existir, a pergunta ficou respondível: **a roda chega a pedir a
abertura?** Rastreando o `IShell` na sequência que avança a tela (`kselect` → `kdown` → `kselect`, e
a variante com `kright`), o rastreio mostra 39 `CreateInstance`, `SendEvent` e `SetTimer` — e
**nenhum** `CanStartApplet` nem `StartApplet`.

E antes disso, um instrumento na entrega de tecla respondeu o que faltava saber: com `AVK_SELECT`
chegando, são **zero tratadores na tela e nenhum formulário atual**. A Z-Wheel **não usa o `IWidget`**
do motor — ela desenha direto —, então a tecla cai no ramo do `if !tratado` e vai para o applet. E o
applet reage: a tela muda.

Ou seja, o caminho de entrada está provado de ponta a ponta:

```text
roteiro de teclas -> core/varredura -> set_key -> flush_keys -> applet -> a tela muda
```

O que falta é **o gesto**, e ele está dentro da lógica da própria roda: ela recebe a tecla, muda de
tela e não pede abertura. Fechar isso é engenharia reversa no módulo dela — o trecho que decide
quando chamar o shell —, ou uma sessão com o controle na mão. Não é falta de instrumento: é falta de
saber qual aperto ela espera.

### O gatilho, lido no desmonte, com endereço

Lendo o tratador da roda (`0x7b310`, pelo desmonte com o `ferramentas/desmonta.py`), o gatilho do
lançamento aparece em duas linhas:

```asm
0x07b4bc  cmp  r0, #0x7000      ; o EVENTO
0x07b4c0  beq  #0x7b51c
0x07b51c  sub  ip, r6, #0x400
0x07b520  subs ip, ip, #0xea    ; e o wParam == 0x4ea
0x07b524  bne  #0x7b574
0x07b53c  ldr  r0, [r5, #0x580] ; o item escolhido
0x07b544  beq  #0x7b640         ; sem item escolhido, não lança
0x07b55c  ldr  r3, [r1, #0x14]  ; IShell::StartApplet
```

Medido, com o evento injetado pelo roteiro (`ZEEBX_ROM_TECLAS="…:e0x7000=0x4ea"`): o evento **chega
ao applet** e o `StartApplet` **não é chamado**, porque a roda não tem item escolhido — a navegação
por tecla muda a tela sem preencher o campo `+0x580` dela.

Duas coisas ficaram de ferramenta: o roteiro sabe entregar evento de widget, e há teste do **texto**
até a ordem (`o_texto_do_roteiro_vira_evento`) e da ordem até a entrega
(`o_roteiro_entrega_evento_de_widget`). Quem continuar daqui pode injetar o gatilho e observar o que
a roda faz, sem repetir a leitura do desmonte.

### O bloqueio, no nível do componente

**Quem manda o `0x7000` no console é o widget da lista**, e a roda **cria** esse widget: entre as
classes que ela pede está `0x01028e19`, que é da família de widgets do motor
(`FAMILIA_DOS_WIDGETS`). O que não existe é o **comportamento** dele: a nossa implementação responde
`Interface::Widget` e para aí. No console, é esse widget que preenche o item escolhido e manda o
evento ao ser ativado.

Isso fecha a explicação do ciclo inteiro:

```text
manche/tecla -> widget da lista (0x01028e19) escolhe o item e escreve [roda+0x580]
             -> widget manda evento 0x7000 com wParam 0x4ea
             -> a roda chama IShell::StartApplet
             -> o shell troca de sessão, e o core já sabe fazer a volta
```

O único elo que falta é o segundo. Sem ele a roda navega, muda de tela e **nunca** chega ao
`StartApplet` — que é exatamente o que as medições mostraram, uma por uma.

**O caminho para fechar**: implementar o comportamento dessa família de widgets a partir do SDK, e
não por tentativa. O que já existe é a rede de segurança: o roteiro injeta o evento, e o relatório
diz se a abertura foi pedida.

## O defeito que invalidou toda a investigação do gesto

O `ZEEBX_ROM_TECLAS` não chegava combinado ao jogo. O pad do roteiro era recriado a cada passo — e,
depois de uma primeira correção, a cada **quadro** —, então:

```text
roteiro "18000:x=128,20000:b1"
   antes:  0x10006b50  00 00 00 00 | 80 00 00 00   ← manche no CENTRO no instante do aperto
   depois: 0x10006b50  00 00 00 00 | ff 00 00 00   ← manche no MÁXIMO, e o aperto não o solta
```

O endereço `0x10006b50` é onde a Z-Wheel guarda o que o `GetPositionState` respondeu: o despejo
mostra o que o jogo **realmente leu**. Segurar o manche e apertar um botão — o gesto que um menu
pede para abrir — nunca chegava inteiro ao guest. **Toda conclusão anterior de que "o gesto não
existe" era inválida**: o instrumento estava quebrado, e o defeito era silencioso porque o pad
funcionava para **um** passo.

Duas coisas ficaram no lugar: `passos_vencidos` deixa o contrato escrito, e com o rastreio ligado
cada passo do roteiro sai no relatório (`roteiro 20003 ms: passo 1 -> x=255 y=128 botoes=0x0`). Um
roteiro que não chega ao jogo agora se distingue de um jogo que o ignora.

Com o instrumento funcionando, a roda **reage ao manche** — a tela muda de 2001 para 1904 cores, e
com o manche mantido por doze segundos vai a 4098. Mas o pedido de abertura **não vem**:

```text
roteiro "20000:x=128" (manche no maximo ate o fim, 32 s de execucao)
  estado: roda
  tela: 4098 cor(es), dominante 0xd69a
  abertura pedida: (nenhuma)
```

O relatório agora diz explicitamente quando o shell pede outro applet (`Session::take_launch_request`,
o mesmo gancho que o core usa para trocar de sessão), então "não pediu" é uma resposta medida e não
uma ausência de pista. Ela navega por **posição** — registra `RegisterForPositionChange`, lê
`GetPositionState` e nunca pede evento de botão —, então o gesto que confirma está na lógica dela, e
não no caminho de entrada. Achá-lo é trabalho de engenharia reversa no módulo, ou uma sessão com o
controle na mão.

É aqui que a investigação headless para, e por um motivo prático: **o instrumento já respondeu o que
tinha de responder** (a entrada chega, a roda reage, o pedido não sai), e o que falta é o significado
do gesto — que só o frontend com controle na mão, ou uma sessão de engenharia reversa da própria
roda, resolve. Fica escrito para quem pegar: os três descartes acima poupam a repetição de três
medições, e a lista de tentativas é a metade do caminho de volta.

## A Z-Wheel navega por TECLA, não pelo controle

Isto foi medido, e explica uma investigação inteira que corria atrás do gesto errado. O motor já
trazia a pista escrita — *"quem recebe tecla primeiro é o widget, não o aplicativo"* —, e a roda
confirma:

| Entrada | Efeito na tela |
|---|---|
| `AVK_SELECT` (`Return`) | sai de 1904 para 6427 cores: muda de tela |
| seta direita + `SELECT` | 1892 cores |
| seta baixo + `SELECT` | 86 cores, dominante branca (tela de carregamento) |
| `AVK_CONFIRMA` (`0xe064`) | 214 cores |
| manche, botões de controle, `AVK_0`, `AVK_CLR` | **nada** |

O manche e os botões não movem um pixel. Não é falta de gesto: é que a lista é um **widget**, e no
BREW widget recebe `EVT_KEY`. Registrado no `IHID` a roda só *lê a posição* — o que levou a uma
conclusão errada por um bom tempo, porque o instrumento de teste não sabia apertar tecla.

Consertado o instrumento (`ZEEBX_ROM_TECLAS` aceita `k0`, `kclr`, `kselect`, `kright`, `kconfirma`),
a navegação apareceu na primeira leva.

### O que a roda abre, e o que não abre

Ela percorre `assets/stage_slides/5008X/slidebanner.qxt` — que é pasta **da própria roda**
(`mod/274755`), não de um jogo — e chama `IShell::CreateInstance` 46 vezes, montando a lista. O
pedido de abertura (`StartApplet`) **não veio** em nenhuma das doze execuções medidas. Ou seja: a
roda avança, reage e desenha, mas ainda não escolheu um jogo — o ciclo não fechou.

### Um defeito real no caminho

```
tectoymain.c:1668  ERROR: Unable to create instance of AEECLSID_LCT_SIMCARDCTL, cannot do SIM check
```

A interface `0x01006c01` já existia e já era atendida; o que faltava era a **fábrica** conhecer a
classe, como já conhecia a vizinha `0x01006c02`. A constante estava definida e nunca usada. Depois
da correção a classe sai da lista de faltantes e o log da roda deixa de acusar o erro — e a mesma
sequência que quebrava com `acesso inválido a 0x0` passou a terminar em `roda`.

Ainda falta, das classes, só `0x01000000`, que o SDK não lista como classe (é `QVERSION`) e que a
roda pede uma vez sem reclamar.

## Como fechar o item 8 (o ciclo da Z-Wheel)

É a única coisa que nenhum teste daqui alcança: a roda **não desenha** no caminho da varredura
(medido: doze segundos com e sem manche diferem em mil instruções de 133 milhões), então só o
RetroArch responde. O roteiro, e o que cada resultado significa:

1. abra a **Z-Wheel** como conteúdo (o `.zip` do pacote, ou o `.mod` de dentro dele);
2. espere a roda aparecer — o core acha **63 jogos** ao lado do conteúdo (medido), então ela não
   pode abrir vazia;
3. **navegue pelo manche** (a roda é navegada pelo analógico, não pelos botões) até um jogo;
4. **confirme** com o botão de ação — o shell então pede a abertura, e o core troca de sessão
   sozinho;
5. jogue alguns segundos e **saia pelo Select** do RetroPad (ele vale como `AVK_CLR`, o "voltar" do
   console) — a volta é para a roda, como no aparelho.

O que observar, e o que cada coisa quer dizer:

| Sintoma | Leitura provável |
|---|---|
| a roda abre e navega, mas nenhum jogo abre | o pedido de abertura não chegou, ou o ClassID não está na pasta de jogos |
| o jogo abre e o Select não volta | o atalho de `AVK_CLR` ou a volta pela sessão anterior |
| a roda abre **vazia** | não é descoberta: o core vê os 63 (medido) — é desenho ou instalação |
| a roda não desenha nada | mesma família do Prey Evil: falta alguma interface gráfica |

A linha do log que interessa em cada caso:

```text
Zeebx: o shell pediu {classe}; abrindo {caminho}
Zeebx: o shell pediu {classe}, que não está na pasta de jogos
```

## Os passos que só rodam no dia da tag

Empacotar e publicar só acontece quando alguém envia uma tag, então esses caminhos nunca eram
exercitados — e três defeitos apareceram quando passei a ensaiá-los à mão:

1. **O `zip` não existe no runner do Windows.** O passo de empacotar o core usava `zip` num
   `shell: bash`, e falharia em dois dos seis alvos. Cada sistema agora usa a ferramenta que tem
   (`Compress-Archive` no Windows).
2. **Os nomes dos artefatos podiam colidir.** O job da release baixa tudo para um diretório só e
   publica `pacotes/*`: seis `zeebx_libretro.so` iguais derrubariam a publicação. Os zips por alvo
   evitam isso, e conferi que são **11 artefatos com nomes distintos**.
3. **O `.deb` não puxava as bibliotecas de janela.** O `eframe` é declarado com `x11` e `wayland`,
   então o binário linka `libX11`, `libXcursor`, `libXrandr`, `libXi` e `libwayland-client` — e a
   lista de dependências não tinha nenhuma delas. Numa instalação de sistema mínimo o emulador
   instalaria e não abriria.

E o ensaio do empacotamento do core confirmou o que importa para o usuário: o `.zip` sai com o
`.so` e o `.info` de mesmo nome, e a biblioteca **carrega depois de extraída** (`api_version` 1).

**A lição que fica:** caminho que só roda em dia de release é caminho sem teste. Ensaia-lo à mão
custou minutos e pegou três defeitos.

## O formato do `.rdb` não é SQLite (e quase "consertamos" um arquivo bom)

Registro porque custa uma investigação: o `.rdb` do RetroArch **parece** banco de dados e a extensão
convida a abri-lo com `sqlite3` — que responde `file is not a database`. Não é defeito: o formato é
o do próprio RetroArch, com a assinatura `RARCHDB\0` seguida de **msgpack**. O nosso arquivo abre
assim:

```text
magic: RARCHDB\0  ·  blocos: 119  ·  titulos distintos: 62
chaves de cada bloco: crc, description, name, rom_name, size
os cinco titulos fora do DAT estao la, com o nome certo
```

O `Mobile - Zeebo.lpl` também já traz os 62, com os cinco nomeados. Ou seja: quem abrir o RetroArch
agora vê os títulos com nome e capa, sem depender de nada do banco.

## Antes de abrir o RetroArch, nada a limpar

Vale registrar porque é um susto comum: o RetroArch guarda um `core_info.cache` com as informações
dos cores, e um cache velho faz ele **recusar** extensões que o `.info` já declara — o que parece
defeito nosso. Conferido nesta máquina:

```text
~/.config/retroarch/cores/zeebx_libretro.info   332 bytes, 21:09
  supported_extensions = "mod|zip|7z"
  database = "Mobile - Zeebo"
core_info.cache: não existe
diff contra o repositório: idêntico
```

Ou seja: não há cache para limpar, e o core instalado é byte a byte o do repositório. Se algum dia
o `.info` mudar e o RetroArch insistir na versão antiga, apagar o `core_info.cache` resolve.

## Como capturar o que o core diz

O core escreve em dois lugares, e os dois servem para diagnosticar sem abrir depurador:

- **na tela**, por `RETRO_ENVIRONMENT_SET_MESSAGE`: recusa de render em hardware, falha ao abrir
  jogo, avisos de acervo. Três segundos, tempo de ler sem atrapalhar quem joga;
- **no log do RetroArch**, por `RETRO_ENVIRONMENT_GET_LOG_INTERFACE`: a mesma informação e mais
  (quantos jogos achou, se desenha na placa, que applet o shell pediu).

Para ver o log sem mexer nas preferências:

```bash
retroarch --verbose 2>&1 | grep -i zeebx
```

O que procurar, em ordem de importância para o item 5:

```text
Zeebx: o frontend aceitou render em hardware (OpenGL 3.3); o desenho passa a ser na placa
Zeebx: desenhando na placa
```

ou, se a tentativa falhar — e aí o jogo continua rodando no processador:

```text
Zeebx: o render em hardware falhou (…); seguindo no processador
```

Qualquer uma das duas linhas é resultado: a primeira diz que o encaixe fechou, a segunda diz onde
ele não fechou.

## Ferramentas que nasceram aqui

```bash
# varredura com relatório por jogo, comparando com a linha de base
ZEEBX_ROM=roms ZEEBX_ROM_MS=6000 ZEEBX_ROM_SAIDA=saida ZEEBX_ROM_BASE=docs/varredura \
  cargo test --release varredura -- --nocapture

# roteiro de controle no relógio virtual (botões e manche)
ZEEBX_ROM_TECLAS="4000:x=-128,10000:b1,11000:" …

# perfil de custo por método de API
ZEEBX_ROM_PERFIL=1 …

# os dois rasterizadores, lado a lado, sem janela
cargo run --release -- sessao "roms/Crash.zip" --seconds=3 --dump=software.bmp
cargo run --release -- sessao "roms/Crash.zip" --seconds=3 --placa --dump=placa.bmp

# o core pela própria ABI, com uma ROM de verdade
ZEEBX_CORE_ROM="roms/Z-Wheel.zip" cargo test -p zeebx-libretro -- --nocapture
```

## O áudio MIDI, do diagnóstico à correção

Resumo do que a frente de áudio MIDI fechou, com os números, e do que ficou aberto. O detalhe
medido está em [07 — Áudio](../implementacao/07-audio.md); aqui fica só o mapa.

**O que estava errado, em três camadas:**

1. **A tabela de timbres** mapeava família de oito programas para uma forma de onda fixa, e 29
   (*overdrive*), 30 (distorcida), 42 (violoncelo) e 48 (cordas) saíam com a **mesma onda** — 23,2%
   das notas do Double Dragon entre as três primeiras. Erro total de centroide contra o soundfont:
   **7635 Hz**, caiu para **701 Hz** depois da correção.
2. **Faltava material de amostra.** Entrou um sintetizador de banco (`rustysynth`, MIT, Rust puro),
   **opcional**: o banco não vem embutido (32 MB), é procurado na pasta do aparelho, e sem ele o
   MIDI volta para a tabela. Nas doze músicas do Double Dragon, o erro de centroide contra a
   referência é **11,3%** com o banco e **40,3%** com a tabela.
3. **O custo de sintetizar era absurdo.** A síntese somava `sin()` por harmônico **em cada amostra**,
   dentro do despacho de API: **6,25 s** por música, ou seja, o jogo travava seis segundos toda vez
   que uma trilha começava. Com a onda pré-calculada por voz: **183 ms** (34×), com o timbre
   intacto.

**O que está validado:** varredura das 62 ROMs com **0 diferenças** de linha de base; quatro
combinações de features verdes (456/461/467/472 + 5 do core); CI **6/6** nos dois workflows, com o
`rustysynth` dentro — e o `readelf` do artefato ARM confirma que as dependências continuam sendo só
libstdc++, libgcc, libm e libc.

**O que estava pendente naquele fechamento:** instalar o core no cartão do muOS. Isso foi feito depois
no SD1 com o core AArch64, o `.info` e o `GeneralUser-GS.sf2`; os hashes e o atraso observado no
primeiro MIDI estão registrados na seção seguinte. O comando continua sendo a referência para uma
instalação limpa:

```bash
python3 ferramentas/instala_core.py --muos /media/$USER/ROOTFS --banco GeneralUser-GS.sf2
```

A decisão de banco continua aberta e não é técnica: o GeneralUser GS chega a 11,3% e permite
redistribuir, mas o texto da licença admite origem desconhecida de parte das amostras. Os
candidatos, com tamanho, licença e presets medidos, estão na tabela do `07-audio.md`.

## SoundFont no RG40XX-H: a primeira música demora, mas toca

**Observação física de 22/09/2026:** no RG40XX-H, o core novo com `GeneralUser-GS.sf2` demorou mais de
**2 minutos** antes de tocar a música tema do menu do Double Dragon; depois subiu e a música foi ouvida
no console. Isso não é uma falha de formato nem de caminho: o banco instalado foi conferido com SHA-256
`9575028c7a1f589f5770fccc8cff273456af40cd26ed836944e9a5152688cfe`, e o core novo com o suporte
`rustysynth` está no SD1.

**Medição no laptop:** `SoundFont::new` levou **66 ms**; a faixa de 47,5 s levou **279 ms** com o
`rustysynth` padrão (bloco interno 64, chorus/reverb ligados). Com bloco 1024 e efeitos desligados,
levou **149 ms**. As doze músicas do Double Dragon renderizaram em cerca de **7,2 s** no laptop.
Isso separa o custo de abrir os 32 MB do custo de renderizar as vozes. No código atual,
`Machine::new` carrega o banco de forma síncrona e `decodifica_som` renderiza a partitura inteira antes
de entregá-la ao mixer; o cache é por caminho e evita recarregar o banco no mesmo processo, mas não evita
a primeira síntese de cada MIDI.

**Hipótese de trabalho:** o atraso no H700 está no custo de renderização síncrona (possivelmente várias
faixas solicitadas no arranque), não na cópia ou no parser do `.sf2`. O core AArch64 foi compilado com
`-C target-cpu=cortex-a35`, a configuração portátil usada para RK3326/H700. Ainda falta medir o tempo
por faixa no aparelho. O A/B previsto é retirar temporariamente o `.sf2`: a tabela de timbres deve
voltar ao arranque rápido e confirmar a regressão do caminho de amostras.

**Otimizações candidatas, sem aplicar ainda:** bloco interno 1024; chorus/reverb opcional no portátil;
cache persistente de PCM; ou renderização incremental/assíncrona em vez de sintetizar a música inteira
no despacho de `Play`. A medição local mostra que bloco/efeitos sozinhos dão cerca de 1,9x, portanto
não são ainda uma explicação completa para os mais de dois minutos.

## O item 8, do "não reage" ao "abre e executa" — o que mudou nesta sessão

O ciclo da Z-Wheel estava dado como bloqueado num caminho sem janela. Ele abre, escolhe, **abre o jogo
e executa**, nos dois caminhos (a varredura e o core Libretro), com o pedido de abertura provado por
`Session::take_launch_request` — e foram **quatro defeitos reais** derrubados no caminho, nenhum deles
o que se procurava:

1. **A classe do `IDownload` (`0x01000000`) era recusada.** O `AEEClassIDs.h` do SDK diz, em três
   linhas seguidas, que `AEECLSID_PRIV` é `QVERSION` e que `AEECLSID_DOWNLOAD` é `AEECLSID_PRIV`; a
   leitura anterior parou na primeira. Recusada, a Z-Wheel abortava a biblioteca de jogos.
2. **O `SystemCtl::slot5` não tinha nome.** Lido no firmware (é o `DefinirModo` com um argumento a
   mais), o confirmar deixou de abortar a varredura.
3. **A classe do cartão SIM estava sendo oferecida.** O comentário longo de `Interface::SimCardCtl`
   já dizia que o `Unable to create instance of AEECLSID_LCT_SIMCARDCTL` **é o caminho certo**; um
   commit de 21/09 a pôs na fábrica para calar o log. A/B: 474 692 instruções e quadro congelado,
   contra **750 milhões** e cinco quadros distintos.
4. **O core não consumia as telas intermediárias do `Update`.** A janela e o `run` consomem; o core
   não — e com a fila cheia o `advance_once` devolvia "apresentou" para sempre, sem rodar o guest e
   sem esvaziar a fila de teclas. Medido: relógio parado em 37 012 ms e **uma** tecla entregue, contra
   78 983 ms e as oito do roteiro, uma a uma.

Com isso, no caminho do RetroArch: `abertura 0x0108e356` (o Alien Breaker), a sessão trocada para o
jogo e 61 814 ms virtuais executados.

**O que falta, com o número:** a **volta à roda** — o core devolve o controle ao shell quando o jogo
termina, e isso não ficou provado. O limite está medido em cinco tentativas: o lançamento só acontece
com o catálogo cheio, e ali o foco cai num jogo que não sai sozinho; com poucos títulos a grade não
oferece o foco que o roteiro alcança. Falta ler a própria grade (o censo do acessador e a árvore de
widgets já existem nos dois caminhos).

**Instrumentos que ficaram no repositório**, úteis para isso e para o aparelho: a captura de serial no
core (`ZEEBX_CORE_SERIAL`), a linha da **entrega de cada tecla** e a da **saída da fila**, o censo do
acessador por classe de widget (`ZEEBX_ROM_SELETORES`), e no teste do core a classe que roda
(`CLASSE_ATUAL`), o relógio virtual (`RELOGIO`), as instruções (`INSTRUCOES`) e o roteiro
configurável (`ZEEBX_CORE_TECLAS=ms:id`).

**Bloqueado por hardware, declarado:** teste físico em R36S/RG40XX-H (item 5) e validação no
aparelho (item 8). O core, o `.info` e o banco MIDI já foram instalados no SD1; o primeiro teste no
RG40XX-H tocou, mas demorou mais de dois minutos para iniciar a primeira música.

