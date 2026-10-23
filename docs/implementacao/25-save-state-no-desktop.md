# 25 — Save state no desktop

A janela do jogo da interface Qt grava e carrega o estado do console em dez slots por jogo, por
atalhos de teclado, como o PCSX2. Até a 0.4.3 só o Android (cinco slots, desde a 0.4.0) e o core
Libretro (pelos slots do RetroArch, desde a 0.3.0) faziam isso. No Qt o estado só era usado pelo
rewind.

O formato do estado não muda: é o ZBXS de [`src/save_state.rs`](../../src/save_state.rs), com
versão, seções nomeadas e `crc32`, o mesmo do Android, do core e do rewind. O que este trabalho
acrescenta é o que fica em volta: onde o arquivo mora, quem grava, e as teclas.

## 1. Os slots

```text
<config>/states/<ContentId>/slot-N.zbxstate   o estado
<config>/states/<ContentId>/slot-N.rgb565     a miniatura, opcional
```

O módulo é o [`src/ui/estados.rs`](../../src/ui/estados.rs). **Nasceu no Android**, como
`frontends/android/src/estado.rs`, e não tinha nada de Android: dar nome aos slots, gravar de forma
atômica, ler a miniatura. Veio para o núcleo, e o Android passou a usá-lo daqui. Os dois gravam no
mesmo layout, e um estado copiado do celular para o computador aparece no mesmo slot.

- **A pasta é o [`ContentId`](../../src/storage.rs)**, o BLAKE3 do arquivo que o frontend abriu
  (o `.zip`, o `.7z` ou o `.mod`), antes da extração. Duas versões do mesmo título não dividem
  estado: um estado é a memória de um binário, e no binário do outro ele não faz sentido. A sessão
  já calculava esse hash para achar a pasta dos saves; agora ela o guarda
  (`Session::conteudo`), e o Qt não lê o pacote inteiro uma segunda vez a cada jogo aberto, nem a
  cada um que a Z-Wheel lança.
- **Dez slots**, como no PCSX2. Eram cinco no Android, que mostra os slots numa lista com rolagem
  e por isso não mudou de desenho. Os `slot-1` a `slot-5` de antes continuam onde estavam.
- **A gravação é atômica**: o temporário com `fsync`, o estado velho para `.bak`, o novo no lugar,
  e o `.bak` apagado. Uma queda no meio deixa só o `.bak`, que a próxima leitura devolve ao lugar.
  Sobrescrever um slot nunca destrói o último estado válido.
- **A miniatura é um arquivo à parte**, de 160×120 no máximo, e é enfeite: estragada ou ausente, o
  slot aparece sem imagem e carrega igual. Gravar o estado e falhar a miniatura é sucesso.
- O estado lido tem teto de 128 MiB, para um arquivo estranho não pedir uma alocação absurda. Os
  medidos têm dezenas de MB.

No desktop, `<config>` é o `config_dir()` (`~/.config/zeebx` no Linux), a mesma raiz dos saves e
do cache.

## 2. Salvar e carregar

O `estados::salva` pede o `Session::pode_salvar` (recusa no meio de um `glBegin` sem `glEnd`),
avisa o espelho do estado da placa (`retoma_o_contexto`: quem divide o contexto com o motor pode
ter pintado nele) e **traz o quadro da placa para a tela antes de gravar**
(`materializa_quadro_gl`). É a lição do rewind, em [24](24-velocidade.md#a-tela-preta-e-o-passo-para-a-frente):
o último quadro fica na textura, a textura não entra no estado, e sem isso o estado carregado
voltava com a tela de um quadro antigo, ou preta num jogo que só desenha pelo GL. O save state do
Android não fazia isso, e agora faz.

**No Qt, as duas pontas rodam com o contexto do jogo corrente** (`no_contexto` no `nucleo.rs`). A
miniatura vem da textura, e a textura só se lê nele. Carregar refaz as texturas na placa, e fora
dele elas iriam parar no contexto do Qt Quick. Foi exatamente o defeito que deixou o rewind sem
funcionar no Qt.

O que a janela faz depois de o console voltar a outro instante mora na `Partida`
(`depois_de_restaurar`), e vale também para o desfazer:

- os pontos do rewind são esquecidos, porque são de outra linha do tempo;
- a entrada já entregue ao jogo é esquecida, e o que está apertado agora chega de novo como aperto
  no primeiro quadro restaurado;
- o tempo real que passou no carregar não é para o jogo recuperar;
- **a pausa fica como estava.** Carregar com o jogo parado mostra o instante carregado, parado.

Na ponte, a chave da imagem que está no item é esquecida: a tela restaurada pode ter a mesma série
e as mesmas escritas que a de antes, e a próxima volta deixaria a imagem velha à mostra.

O que o jogo gravou no disco não volta, como no rewind: carregar um estado de antes de um save não
desfaz o save.

## 3. Desfazer carregar

Um carregar sem querer joga fora o progresso desde o último save, e é o acidente que custa caro.
Antes de cada carregar, a `Partida` grava o estado de agora **na memória**
(`antes_de_carregar`). O atalho de desfazer o devolve, uma vez só: desfazer de novo não refaz. Não
há arquivo novo no disco, e a cópia some com a partida.

Se o `pode_salvar` recusa no instante do carregar, o carregar segue sem desfazer: o jogador pediu.
Um desfazer que o motor recusasse (só por defeito dele, porque o estado é da mesma sessão) deixa a
cópia guardada para uma segunda tentativa.

Sobrescrever um slot não tem desfazer. O aviso na tela diz o slot, e o PCSX2 faz o mesmo. Uma cópia
do arquivo anterior dobraria o espaço em disco dos slots usados, e ficou de fora.

## 4. Os atalhos

| | Windows e Linux | macOS |
|---|---|---|
| Salvar no slot atual | F1 | ⌘S |
| Carregar do slot atual | F3 | ⌘L |
| Próximo slot | F2 | ⌘] |
| Slot anterior | Shift+F2 | ⌘[ |
| Desfazer carregar | F4 | ⌘Z |

Fora do Mac é o PCSX2. **No Mac as teclas F são de brilho, de volume e do Mission Control**, e só
chegam ao programa com o Fn; o F11 mostra a mesa. Lá o padrão é o costume do sistema: ⌘S e ⌘L são o
gravar e o abrir de todo programa, e o OpenEmu faz o mesmo. Nenhum jogo usa o ⌘, então nada colide
com os botões. Os atalhos de antes (F8 a F11) não mudaram, e têm no Mac o mesmo problema.

Os cinco são só de teclado, como o screenshot e o reiniciar: um botão de controle sozinho que grava
ou carrega estado é fácil de apertar sem querer, e o controle do Zeebo já usa quase todos. O jeito
certo é uma combinação com um botão de atalho segurado, como o *Hotkey Enable* do RetroArch, e ela
serviria a todos os atalhos.

### A combinação

Os atalhos de antes guardam uma tecla só, pelo nome do `egui::Key`. Os do save state guardam uma
[`Combinacao`](../../src/ui/settings.rs): a tecla com os modificadores na frente, em texto
(`Shift+F2`, `Cmd+S`). Uma tecla sozinha é escrita como sempre foi, e os nomes do egui não têm `+`
(o da tecla de mais é `Plus`).

**A combinação bate só com os mesmos modificadores.** O F2 anda para a frente e o Shift+F2 para
trás; aceitar modificador a mais faria o Shift+F2 andar para a frente também. Por isso a janela do
jogo pergunta pelos atalhos do save state **antes** dos outros: o screenshot olha só a tecla, e um
Shift+F9 de save state não pode virar o F9 do screenshot.

**O Qt troca os nomes no Mac**: o que ele chama de `ControlModifier` é o ⌘, e o `MetaModifier` é
o Control. O `modificadores_do_qt` da ponte desfaz a troca. Fora do Mac o Meta é a tecla do sistema
(a do Windows, o Super), guardada no mesmo campo que o ⌘. A tela mostra os símbolos do Mac lá
(`⌘]`) e `Shift+F2` fora dele.

A aba **Atalhos** recusa o que já recusava, olhando os modificadores: o Esc, o P e o F11 sozinhos
e o Alt+Enter são da janela do jogo; a mesma combinação não serve a dois atalhos; e a tecla
sozinha de um botão do Zeebo é recusada, como no screenshot. Com modificador ela não colide com
botão nenhum, porque o jogo não lê modificadores.

### Quem já tinha F1 num botão

O F1 a F4 não estão em nenhum botão do teclado de fábrica, mas quem remapeou pode tê-los posto
num, e o atalho, perguntado antes do jogo, roubaria o botão. É o caso do R de reiniciar, e a regra
é a mesma (`Settings::adota_os_atalhos_novos`): um `settings.json` que ainda não conhecia o atalho,
com a tecla de fábrica dele num botão (nas gerais ou nas de algum jogo), recebe o atalho vazio. Uma
combinação com modificador chega sempre.

## 5. Como se conferiu

```bash
XDG_CONFIG_HOME=$(mktemp -d) ZEEBX_TESTE_ROM=caminho/para/jogo.mod \
  cargo test --release -p zeebx --lib o_save_state_vai_e_volta -- --nocapture
```

Os dois testes (`ui::partida`) abrem o jogo, andam três segundos, salvam no slot 5, andam mais
dois, carregam, desfazem e andam de novo. A cada passo conferem o relógio do jogo e a assinatura da
tela, um pelo processador e o outro pela placa, num contexto fora de tela. O `XDG_CONFIG_HOME`
importa: a sessão grava com o perfil de `config_dir`, e sem ele o teste mexeria nos saves de
verdade.

No Double Dragon, a tela do instante salvo e a de antes do carregar são diferentes (3083 ms e
5249 ms pelo processador; 3216 ms e 5216 ms pela placa), e cada uma voltou no seu passo. No Magical
Drop III as duas telas são iguais nesses instantes, e ali só o relógio prova alguma coisa.

Os testes de `ui::estados` cobrem o disco sem jogo nenhum: o slot que vai e volta, o slot fora da
conta, o estado com um byte trocado (recusado pelo `crc32`), a miniatura estragada e o `.bak` de
uma gravação interrompida.

**Não conferido**: as teclas numa janela de verdade e o ⌘ num Mac. O ⌘] e o ⌘[ dependem de onde o
teclado põe os colchetes, e num teclado que não é americano (o ABNT2 do Mac, por exemplo) o Qt pode
entregar outra tecla com o ⌘ segurado.

## O que falta

- O painel com as miniaturas e a data de cada slot, no menu Sistema da repaginada da interface.
- O gerenciador de saves (`JanelaDeSaves.qml`) não lista os estados, e por ele não se apagam.
- O botão de atalho segurado, para os atalhos aceitarem botão de controle.
