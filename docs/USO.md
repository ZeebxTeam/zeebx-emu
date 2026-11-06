# Usando

Sem argumentos, abre a interface. Pela linha de comando:

```bash
cargo run --release -- run "roms/Quake.zip" --window
```

Zips são extraídos para um cache e o `.mod` certo é escolhido sozinho. `--seconds=N` define
quantos segundos de tempo virtual emular quando não há janela; com janela, roda até você fechar.

## Controles do teclado

| Tecla | Controle do Zeebo |
|---|---|
| Setas | direcional |
| Z, X ou Espaço, C, V | botões 1, 2, 3 e 4 |
| Q, W | ZL e ZR |
| F, G | analógico esquerdo, direito |
| H, Backspace, Enter | HOME |

## Relatório de depuração

O `run` informa onde o jogo parou, o que ele pediu e não temos, e o log que os próprios
desenvolvedores deixaram no binário — por `DBGPRINTF` e por semihosting do ARM. Esse relatório é
o backlog do projeto. As opções de depuração estão em [ARCHITECTURE.md](../ARCHITECTURE.md).

## Com um frontend seu

Para integrar o emulador a um frontend próprio, ver [HEADLESS.md](HEADLESS.md).