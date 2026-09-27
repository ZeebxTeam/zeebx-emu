# 22 — Screenshots

Um atalho na janela do jogo (F9, trocável) grava o quadro que está na tela num `.png`. A régua é a
das [diretrizes de screenshot do RetroAchievements](https://docs.retroachievements.org/guidelines/content/screenshot-guidelines.html):
a imagem vem do próprio emulador, na resolução em que o jogo foi desenhado, sem filtro, sem marca
d'água e sem ampliação de janela. O que o site aceita além do nativo é o 3D renderizado maior
**pela resolução interna do emulador**, que é como o DuckStation e o PPSSPP fazem.

Só a janela Qt tem o atalho. A escolha do quadro e a gravação moram no núcleo
(`src/ui/screenshot.rs` e `Session::captura`), sem nada de janela, para o Android poder ganhar
um botão depois sem reescrever nada.

## 1. Que quadro sai

**O quadro que a janela mostra, no tamanho em que ele existe.** São dois:

| Na tela | O que sai |
|---|---|
| O 3D na placa, sem nada desenhado por cima em 2D, com a resolução interna acima de 1x (ou a proporção larga ligada) | A textura da placa, lida inteira: 1280×960 em 2x, 1920×1440 em 3x |
| Todo o resto: jogo 2D, HUD pelo `IDisplay`, caixa de mensagem, a Z-Wheel, o rasterizador de software, a resolução interna em 1x | A tela do console, **640×480**, que é a resolução do Zeebo |

A regra de qual dos dois está na tela é a do `Session::quadro_na_placa`, a mesma que o
`Nucleo::quadro` do Qt usa para escolher entre textura e imagem. **Não serve chamar o
`quadro_grande` direto**: ele lê a placa sem perguntar se houve desenho 2D depois do último
`eglSwapBuffers`, e num jogo com HUD o print sairia sem o HUD. O HUD é composto na CPU, sobre a
tela de 640×480, e não existe versão grande dele. Por isso, nesse caso, sai o nativo, e não uma
ampliação: ampliar por vizinho mais próximo não acrescenta detalhe nenhum, e o RetroAchievements
não aceita.

Conferido pelo `zeebx sessao --placa --escala=3 --fotos=…`, que diz a cada foto se a janela
mostraria o quadro grande: nas três fotos do Alien Breaker Deluxe (3D na placa) o print saiu em
1920×1440, e nas três do Action Hero 3D saiu em 640×480. Esse último desenha o 3D pelo motor
IMICRO3D, em software, na tela 2D (zero quadros de GL até os 19 s), e o `quadro_grande` sozinho
teria devolvido um 1920×1440 que não era o que estava na tela.

### A resolução configurada vale, mesmo acima de 3x

O site diz "nunca 4x ou mais". O atalho é do emulador, e não só do site: quem pôs 6x quer ver 6x.
Quem vai enviar ao RetroAchievements escolhe 2x ou 3x nas configurações, como faria no
DuckStation. O limite de 6 MB do site não aperta em 3x: a tela de título do Alien Breaker Deluxe
em 1920×1440 deu um PNG de 2,2 MB. Acima disso não foi medido. Reduzir um quadro de 6x para 3x
misturaria pixels, que é justamente a mudança de imagem que a régua proíbe; e renderizar um
quadro extra só para o print levaria um quadro para valer.

A proporção larga experimental segue a mesma lógica: o print sai **com os lados**, do jeito que
está na tela. O site proíbe widescreen, mas a proporção é escolha de quem configura, como a escala.

A superfície esticada (o Quake desenha em 320×400 e a Qualcomm estica para 640×480) sai na
proporção da superfície: 960×1200 em 3x. É o "aspect uncorrected" que o site pede por padrão.

### Oito bits por canal, sem alfa

A tela do console é RGB565, e é assim que o Zeebo a mostra: no nativo, cada canal é expandido
repetindo os bits altos (`0b11111` vira 255, e não 248), como no `Framebuffer::to_argb`. O quadro
da placa é desenhado em oito bits, e o `Session::quadro_grande` o devolve como `Framebuffer`,
ou seja, RGB565: gravar por ele jogaria fora dois ou três bits por canal do que a placa já tinha
calculado, e faixas de degradê apareceriam no céu. O print lê o RGBA cru (`Session::captura`).

O PNG é RGB, sem canal alfa. O alfa da textura não é informação de imagem: o Double Dragon e
os outros da Data East limpam o fundo com alfa zero, e foi isso que vazou pela janela do Qt (ver
"o alfa do quadro" em [`21-migracao-para-qt.md`](21-migracao-para-qt.md)). Gravado como veio, o
fundo desses jogos sairia transparente no print.

## 2. Onde grava

```
<pasta>/<jogo>/<jogo> - AAAA-MM-DD HH-MM-SS.png
```

- **A pasta** é `config_dir()/screenshots/` por padrão (`~/.config/zeebx/screenshots` no Linux),
  e pode ser trocada na aba Geral (`screenshots_dir` no `settings.json`). O nome é em inglês, ao
  contrário de `relatorios/` e `aparelho/`, porque é o que quem vem do RetroArch reconhece.
  `~/.config` é escondida, e por isso a aba tem o botão de abrir a pasta.
- **Uma subpasta por jogo**, com o título da biblioteca (`Nucleo::titulo`): quem tira dez prints
  de cada jogo acha tudo junto. Na falta de título, `Zeebx`.
- **O título perde o que o Windows recusa** (`< > : " / \ | ? *` e os caracteres de controle), e
  os pontos e espaços do fim, que o Windows também apaga em silêncio. O mesmo arquivo tem de
  poder ir de um sistema a outro.
- **A hora é a local, e vem do Qt** (`Qt.formatDateTime` no QML). O núcleo não tem fuso horário
  nenhum: o relógio do guest é GMT, e puxar uma dependência de data só para o nome de um arquivo
  seria desproporcional.
- **Dois prints no mesmo segundo** ganham ` (2)`, ` (3)`… A reserva do nome é o
  `create_new` do sistema de arquivos, e não uma consulta antes: dois F9 seguidos gravam em
  threads diferentes, e "o nome está livre?" seguido de "cria" deixaria as duas escolherem o
  mesmo.

## 3. O atalho

**F9 por padrão, trocável** na seção Atalhos, no fim da aba Controles (`atalhos.screenshot` no
`settings.json`, com o nome de tecla do `bindings`). Esc, P e F11 continuam fixos: a struct
`Atalhos` nasce com um campo só, mas é o lugar deles se um dia forem trocáveis.

- **Só teclado.** O controle do Zeebo não tem tecla livre óbvia, e um combo (Select+algo) exigiria
  uma tela de atalhos por botão que não existe.
- **A captura do atalho recusa** Esc, P e F11, e recusa uma tecla que já é botão do Zeebo em
  alguma porta, dizendo qual ("F9 já é Botão 1 da porta 1"). Trocar as duas de lugar mexeria num
  mapeamento sem o usuário ver. Pelo mesmo motivo, a captura de um botão do Zeebo recusa a tecla
  do atalho.
- **Na janela do jogo, o atalho ganha.** Um `settings.json` antigo pode ter o F9 como botão; o
  `Jogo.qml` pergunta primeiro se a tecla é o atalho, e o jogo não a recebe.
- **A repetição automática não conta.** Segurar o F9 tira um print, e não trinta.

## 4. O aviso, e a thread

Depois do print, um aviso no canto da janela do jogo por dois segundos, no estilo do aviso de
calibração: "Screenshot salvo como: Quake - 2026-09-25 14-03-07.png". Só o nome do arquivo, e não
a resolução: quem escolheu a resolução interna já sabe qual é, e a pasta se abre com um clique no
aviso. Uma falha (pasta sem
permissão, disco cheio) aparece em laranja com o motivo, e o caminho sai no `stderr`. Não vai à
janela de log: ela mostra o `Session::log`, que é do jogo, e o frontend não tem onde escrever lá.

- **O aviso não entra no PNG.** Ele é desenhado pelo Qt por cima do quadro, e o quadro vem do
  núcleo. O site proíbe marca d'água, e é por isso também que não há flash branco: ele apareceria
  num vídeo gravado por fora.
- **A leitura é na thread da interface; a compressão, não.** A textura só pode ser lida com o
  contexto de GL corrente, e o contexto é da thread da interface (ver o topo do `nucleo.rs`). O
  PNG vai para uma thread própria, e a resposta volta por canal na volta seguinte do jogo. Quanto
  custa comprimir um 3840×2880 não foi medido: a thread é precaução, não número.

## 5. O que ficou de fora

- **O egui**, que sai na fase 8 da migração; **o core Libretro**, porque o RetroArch já tem o
  próprio print e a regra da casa é que quem entrega o vídeo é o frontend; **o headless**, que já
  grava um `.png` por quadro (`despejo.rs`); e **o Android**, que não tem F9.
- **A miniatura do save state no Android** (`frontends/android/src/estado.rs`) usa o
  `quadro_grande` sem a pergunta do HUD, e pode sair sem ele. Não é deste trabalho; fica anotado.

## Nota para a próxima versão

As notas moram em `docs/patch-notes/` e são escritas na hora da release. O que esta entra dizendo:

> - **Screenshots.** F9 na janela do jogo grava a tela em `.png`, numa pasta por jogo. Com o 3D
>   na placa e a resolução interna em 2x ou 3x, a imagem sai nesse tamanho; no resto, sai em
>   640×480, a resolução do Zeebo. O atalho e a pasta se trocam nas configurações. Quem tinha o
>   F9 como botão do controle: na janela do jogo ele agora tira screenshot — troque o atalho ou o
>   botão.
