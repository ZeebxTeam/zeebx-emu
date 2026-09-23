# O Zeebx sem interface

O núcleo é o mesmo. O que muda é que não há interface nenhuma por cima dele.

Existe para quem já tem um frontend. Um programa que desenha a carcaça do console, uma estante
de jogos, um gabinete de fliperama — nenhum deles quer a biblioteca do Zeebx aparecendo sobre a
tela que ele mesmo montou. Este binário abre o jogo que lhe derem, obedece a um `config.ini` e
não mostra mais nada.

```
zeebx-headless [OPÇÕES] JOGO
```

O `JOGO` é obrigatório: um `.mod`, ou o `.zip` que o contém. **Não há padrão para ele**, e é de
propósito — este binário é chamado por outro programa, que sabe o que quer abrir, e adivinhar
alguma coisa quando ninguém disse nada seria abrir o que ninguém pediu. Para começar pela
Z-Wheel, passe a Z-Wheel: ela é um jogo como outro qualquer.

O `[system] z_wheel` do arquivo é outra coisa: é para onde o console **volta** quando um jogo
sai sozinho, como no aparelho de verdade. Sem ele, um jogo que sai encerra o emulador — que é o
que um frontend de fora costuma querer, já que a tela dele é que volta a aparecer.

| Opção | O que faz |
|---|---|
| `--config=CAMINHO` | o arquivo a usar; sem isto, procura um `config.ini` ao lado do executável e depois na pasta de configuração do sistema |
| `--controllers` | lista os controles que o sistema enxerga, **com os nomes que o `config.ini` espera** |
| `--example` | escreve na saída padrão um `config.ini` comentado, com os valores de fábrica |
| `--help`, `--version` | o que se espera delas |

Não há mais opções, e é de propósito: o resto está no arquivo. Um frontend que precisa de dois
perfis usa dois arquivos e dois `--config=`.

## A configuração

As chaves e os valores são em **inglês**, como os comandos; os comentários do arquivo, não. A
ideia é que quem escreve um frontend leia a configuração sem precisar de português, e quem edita
o arquivo à mão tenha a explicação na língua do projeto.

Gráficos, áudio e mapeamento de controle **não são um modelo novo**: são os mesmos campos que a
interface do desktop grava no `settings.json`, chegando por INI porque é o que se edita à mão.
Os nomes de tecla e de botão são os mesmos dos dois lados, então um mapeamento copiado da tela
de controles do desktop vale aqui sem tradução — e aquela tela continua sendo o jeito mais fácil
de descobrir como um botão se chama.

Tudo é opcional. Uma chave que falta vale o padrão, e uma chave escrita errada **aparece na
saída de erro** em vez de ficar em silêncio no padrão — que é o único jeito de quem editou o
arquivo descobrir que `resolucao_intern` não é `resolucao_interna`.

Na primeira execução, quando não há arquivo nenhum, ele escreve um de fábrica na pasta de
configuração do sistema e diz onde:

```
config: não havia nenhum; escrevi um em /home/voce/.config/zeebx/config.ini
```

Nunca por cima de um que exista — nem quando o arquivo está lá e só não abriu, porque um
`config.ini` cheio de ajustes é trabalho de quem o escreveu. Com `--config=CAMINHO` apontando
para o que ainda não existe, é **naquele** caminho que ele nasce: a pessoa nomeou o arquivo que
quer.

O arquivo vai para a pasta de configuração e não para o lado do executável, que é o primeiro
lugar da procura: ali a cópia instalada costuma não ter permissão de escrita, e uma que tenha
seria um arquivo dentro da instalação, que some na próxima atualização.

Ele sai **completo**, com tudo escrito e nada a adivinhar — inclusive os treze botões de cada
porta. O bloco das portas é gerado a partir do `Controls::default()` do núcleo, não escrito à
mão, e cada linha diz o que já valeria se não existisse; um teste fixa essa igualdade, porque um
exemplo escrito à mão envelhece calado no dia em que um padrão muda, e um arquivo que mente
sobre o padrão é pior que um arquivo sem a linha. `--example` imprime o mesmo texto na saída
padrão, para quem quer olhá-lo sem criar nada.

Nada é adivinhado. O núcleo sabe procurar uma Z-Wheel sozinho — pela pasta de ROMs e, na falta
dela, por nome dentro da pasta pessoal —, e isso faz sentido na interface, onde é um chute
simpático para quem ainda não configurou nada. Aqui não: o headless usa só o que o arquivo
apontar.

O [`config.ini`](config.ini) deste diretório é o exemplo, comentado linha a linha, e é o que
`--example` imprime. Um teste confere que ele é lido sem nenhum aviso: um exemplo que reclama
ensina errado.

## Os dois modos de vídeo

**`mode = window`** (ou `fullscreen`) abre uma janela com o jogo e nada mais. O contexto de
OpenGL é o dela, e é ele que a sessão usa para rasterizar o 3D quando `gpu_rasterizer`
está ligado: o quadro que a placa acabou de preencher vai à tela sem voltar à CPU. `Esc` fecha,
e isso não é mapeável — quem rodou precisa de uma saída que não dependa de o arquivo estar
certo.

**`mode = none`** não desenha nada. Os quadros saem pela seção `[dump]`: na saída
padrão (`zeebx-headless jogo.mod | seu-frontend`), num arquivo, num FIFO — e aí nada passa por
disco — ou como um `.png` por quadro. Um quadro só é escrito quando muda. Sem janela não há
teclado, porque não há foco; quem joga usa um controle, que o sistema entrega sem precisar
dele. O 3D na placa continua possível: o núcleo abre um contexto fora de tela, o mesmo que o
`zeebx run` usa para medir.

Nos formatos crus o quadro é sempre o do console, 640×480, do começo ao fim. É a única escolha
possível: o fluxo não tem cabeçalho, e um quadro que mudasse de tamanho no meio faria quem conta
bytes do outro lado perder o passo para sempre. Por isso `internal_resolution` e a proporção larga
não chegam ao despejo cru — elas valem na janela. O `.png` é a exceção, porque cada arquivo diz
o próprio tamanho.

## Nos três sistemas

Linux, Windows e macOS. O `release.yml` monta um `zeebx-headless-<sistema>.zip` para cada um,
com o binário, o `config.ini` comentado e este arquivo — o `config.ini` vai junto porque o
primeiro lugar em que o emulador o procura é **ao lado do executável**, que é o que faz uma
cópia portátil funcionar sem tocar na máquina.

Uma diferença que vale saber: no modo `none` com `gpu_rasterizer` ligado, o 3D
precisa de um contexto de OpenGL sem janela, e isso é EGL. No Linux ele está sempre lá; no
Windows vem com o driver que o instala — a NVIDIA instala — ou com uma ANGLE (`libEGL.dll` e
`libGLESv2.dll`) ao lado do executável; no macOS não existe. Onde ele falta, o emulador diz o
motivo e segue no rasterizador de software. **Com janela isso não se aplica**: ali o contexto é
o da própria janela, e os três sistemas o têm.

## Os arquivos

| Arquivo | O que é |
|---|---|
| `src/main.rs` | os argumentos, o modo sem janela e por onde cada modo entra |
| `src/ini.rs` | o leitor de INI, escrito à mão porque são três regras |
| `src/config.rs` | o `config.ini` virando o `Settings` que o núcleo já entende |
| `src/console.rs` | o console rodando: a sessão, a entrada, o relógio e a cadeia da Z-Wheel — o que os dois modos têm em comum |
| `src/janela.rs` | a janela crua sobre winit e glutin, e o laço dela |
| `src/entrada.rs` | o teclado do winit e os controles do host virando o estado das portas |
| `src/despejo.rs` | o quadro saindo sem janela |

## Por que não o eframe

Pelo mesmo motivo que o frontend de Android não o usa: o eframe gira um laço de interface, e
aqui não há interface — há um quadro. O laço é nosso, sobre o winit, e é o que permite tratar a
entrada como o sistema a entrega e ser dono do ciclo de vida da janela.

Isso é sobre o laço, **não** sobre o tamanho da compilação. O `zeebx` de que este pacote depende
é a biblioteca inteira, e fora do Android ela traz o eframe, o `rfd`, o Discord e o `ureq` por
conta própria: compilar o headless compila aquilo junto. Enxugar de verdade seria pôr as telas
do desktop atrás de uma feature do núcleo, o que ainda não foi feito.

O `egui` continua na lista de dependências, mas só pelos tipos: `egui::Key` é o nome canônico de
uma tecla no `settings.json`, e o `ViewportInPixels` é o que o pincel do núcleo (`ui::gpu`)
recebe. Nada é desenhado com ele, e, por já estar na árvore por causa do núcleo, não acrescenta
compilação nenhuma.
