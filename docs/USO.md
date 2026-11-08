# Usando

A interface Qt é o `zeebx-qt`. A linha de comando é o `zeebx`: sem subcomando, ele imprime o uso
e sai.

```bash
cargo run --release -p zeebx-cli -- run "roms/Quake.zip" --window
```

Zips são extraídos para um cache e o `.mod` certo é escolhido sozinho. `--seconds=N` define
quantos segundos de tempo virtual emular quando não há janela; com janela, roda até você fechar.

## Controles do teclado

| Tecla | Controle do Zeebo |
|---|---|
| Setas | direcional |
| Z | botão 1 |
| X ou Espaço | botão 2 |
| C | botão 3 |
| V | botão 4 |
| Q, W | ZL e ZR |
| F | clique do analógico esquerdo (L3) |
| G | clique do analógico direito (R3) |
| H, Backspace, Enter | HOME |

F e G clicam os analógicos (L3 e R3). Elas não os movem. O teclado padrão não tem eixo de analógico.

## Relatório de depuração

O `run` informa onde o jogo parou, o que ele pediu e não temos, e o log que os próprios
desenvolvedores deixaram no binário — por `DBGPRINTF` e por semihosting do ARM. Esse relatório é
o backlog do projeto. As opções de depuração estão em [ARCHITECTURE.md](../ARCHITECTURE.md).

## Com um frontend seu

Para integrar o emulador a um frontend próprio, ver [HEADLESS.md](HEADLESS.md).