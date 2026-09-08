# Zeebx

Emulador do Zeebo, o console que a TecToy lançou em 2009 no Brasil e no México.

O Zeebo era digital-only: os jogos vinham da loja da TecToy, que saiu do ar. Sobraram cerca de
60 títulos que só rodam em quem ainda tem o aparelho ou por meio de modding no console. 
Este projeto existe para ajudar a preservar essas pérolas que fizeram parte da nossa história.

Em desenvolvimento. Hoje 48 dos 62 títulos de teste passam do carregamento e desenham.

## Links úteis

[Servidor Discord: https://discord.gg/D96HjsKTPa](https://discord.gg/D96HjsKTPa)

[GitHub: https://github.com/ZeebxTeam](https://github.com/ZeebxTeam)

## Como funciona

Emular Zeebo não é emular um console: é reimplementar o Qualcomm BREW 4.0.2. O jogo é um binário
ARM que nunca toca hardware — ele chama interfaces do sistema por tabelas de ponteiros. Então o
caminho é executar o código ARM num núcleo emulado e atender cada chamada de API no host.

As vtables que entregamos ao jogo apontam para endereços que **não existem** no mapa de memória.
Quando o jogo chama um método, o núcleo aborta a busca de instrução e o endereço nos diz qual
interface e qual método foram pedidos. Não há stub, nem código de cola.

O desenho completo está em [ARCHITECTURE.md](ARCHITECTURE.md).

## Compatibilidade

Poucas ROMs ainda rodam sem problemas, diversos jogos podem apresentar travamentos antes da inicialização ou durante a execução.

O estado de cada título, com os endereços de cada parada, está em
[docs/implementacao/11-compatibilidade.md](docs/implementacao/11-compatibilidade.md).

## Compilando

Rust 1.88 ou mais novo.

```bash
cargo build --release
```

## Usando

Sem argumentos, abre a interface. Pela linha de comando:

```bash
cargo run --release -- run "roms/Quake.zip" --window
```

Zips são extraídos para um cache e o `.mod` certo é escolhido sozinho. `--seconds=N` define
quantos segundos de tempo virtual emular quando não há janela; com janela, roda até você fechar.

Os controles no teclado:

| Tecla | Controle do Zeebo |
|---|---|
| Setas | direcional |
| Z, X ou Espaço, C, V | botões 1, 2, 3 e 4 |
| Q, W | ZL e ZR |
| F, G | analógico esquerdo, direito |
| H, Backspace, Enter | HOME |

O `run` informa onde o jogo parou, o que ele pediu e não temos, e o log que os próprios
desenvolvedores deixaram no binário — por `DBGPRINTF` e por semihosting do ARM. Esse relatório é
o backlog do projeto. As opções de depuração estão em [ARCHITECTURE.md](ARCHITECTURE.md).

## Plataformas

Linux, Windows e macOS. Mobile está fora do escopo por enquanto.

## Jogos

O repositório não distribui jogos. Coloque os seus em `roms/`, que é ignorada pelo git ou em qualquer outra pasta, e defina nas configurações do programa.

## Documentação

- [ARCHITECTURE.md](ARCHITECTURE.md) — como o emulador é feito
- [docs/](docs/README.md) — a pesquisa: o console, a plataforma BREW, os formatos de arquivo e o
  estado da arte da emulação de Zeebo
- [docs/implementacao/](docs/implementacao/README.md) — cada subsistema, com as decisões e o
  porquê de cada uma
- [TODO.md](TODO.md) — o diário de bordo: o que está pronto, o que falta e o que já custou caro

## Licença

GPL-2.0, o texto completo em [LICENSE](LICENSE).


## Menções

- **tripleoxygen** — engenharia reversa de hardware e firmware do Zeebo, e o material público
  que torna este projeto possível :)