# Proposal for inclusion in the libretro DAT database

Five Zeebo titles from the collection **are not** in the `Mobile - Zeebo.dat` that RetroArch uses
to name the ROMs and to find the covers. Without an entry in the DAT, these titles show up with the
file name and have no cover, even with the dump in hand.

Measured: the DAT in use has **57 titles** and **none** of these five.

| Title | Why it is missing |
|---|---|
| Bad Dudes vs. DragonNinja | official game, released on the console and missing from the database |
| Caveman Ninja | same |
| Dark Seal | same |
| Karnov's Revenge | same |
| Kingdom Hearts V CAST | homebrew, chapter 1 + Agrabah |

## What this directory has

- `Mobile - Zeebo (proposta).dat` — the blocks ready to append to the database's DAT, in **its
  format**: a `clrmamepro` header with the same four fields as the official file (`name`,
  `description`, `version` in quotes in the `YYYY.MM.DD` format, `homepage`), and one `game` per
  title, with `region "Brazil"` and the hashes of each file.
- `proposta.txt` — the same, for human reading: package, size, CRC32 and SHA1 of the `.zip`, and
  every file of the module with size, CRC32, MD5 and SHA1.

The file names follow the database's convention: No-Intro writes `mod274259font.bar` for what is
in `mod/274259/font.bar`. Each `game` carries **several** `rom` entries on purpose: which one is the
dump is the database maintainers' decision, and the proposal carries all the hashes instead of
betting on one.

## Where to send it (checked against the GitHub API, not assumed)

| Destination | State | What to do |
|---|---|---|
| `datomatic.no-intro.org` (`?page=download&op=daily`) | it is the **source** of the database | this is where the title goes in first, with the dump attached |
| `libretro/libretro-database` | 53 DATs in `dat/`, no `Mobile - Zeebo.dat` | PR adding the file |
| `robloach/libretro-dats` | it is the **builder** (Node.js), `database/` is a submodule of the one above | does not take DATs by hand |
| `libretro-thumbnails/Mobile_-_Zeebo` | **does not exist** (404) | the 58 covers go along with the request to create the repository |

The name of the covers repository is the database name with `_` in place of the spaces — checked
against a repository that exists: `libretro-thumbnails/Nintendo_-_Nintendo_Entertainment_System`.

## The covers

There are 58, in PNG, in the format the thumbnails repository accepts (it **rejects** JPEG), in the
`Named_Boxarts/` layout with the exact name of each title in the DAT. Measured: `Named_Boxarts` with
58 PNGs and `Named_Titles` with 44, in the folder RetroArch already uses.

The covers are **not** versioned here: they can be regenerated and weigh a few megabytes. To
rebuild them, `tools/catalog.py --roms <zips> --out <dest> --png`.

## What is missing

Only the submission, which needs the accounts of whoever maintains the fork. Nothing else.
