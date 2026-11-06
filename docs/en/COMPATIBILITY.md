# Overall status

Version considered in this analysis: v0.4.3

| Status | Meaning | Games | % |
|---|---|---:|---:|
| ✅ | Compatible | 55 | 91.7% |
| 🔄 | Works with issues | 4 | 6.7% |
| ❌ | Incompatible | 1 | 1.7% |

## Notes

- We recommend configuring your controller the first time you open the emulator. Some games may still interpret inputs differently.
- Boomerang motion-control games, such as Crash Nitro Kart 3D and the Zeebo Sports titles, can be played with Bluetooth motion controllers such as DualShock 4, DualSense, Wii Remote, and Switch Pro Controller. Motion sensing over Bluetooth currently works on desktop Linux; Libretro cores and other platforms do not yet support it ([#60](https://github.com/ZeebxTeam/zeebx-emu/issues/60)).
- Data East games run in their own built-in arcade emulator.
- All games, whether official, homebrew, or ported, may contain bugs, crash, or remain incompatible. Reports are welcome.
- Zeeboids online features may not always work while they remain under development. Avatars may be deleted after development and migration to a suitable production server; feel free to test them in the meantime.

## Game compatibility list

| Game | Status | Notes |
|---|---|---|
| Action Hero 3D - Wild Dog and IMICRO3D | ✅ | Playable; menus, story, and level |
| Alice in Wonderland | ✅ | Playable; reaches the level |
| Alien Breaker Deluxe | ✅ | Playable |
| Alpine Racer | ✅ | Minor visual and audio issues; fully playable |
| Armageddon Squadron | ✅ | |
| Bad Dudes vs. DragonNinja | ✅ | Looks for `baddudes.pkg`; the package contains `baddudes.zip` |
| Bejeweled Twist | ✅ | |
| Caveman Ninja | ✅ | Built-in arcade emulator, with sound |
| Crash Bandicoot Nitro Kart 3D | ✅ | |
| Dark Seal | ✅ | Built-in arcade emulator, with sound |
| Disney All Star Cards | ✅ | Playable |
| Double Dragon | ✅ | |
| FIFA 09 | ✅ | |
| Galaxy on Fire | ✅ | |
| Heavy Barrel | ✅ | Built-in arcade emulator, with sound |
| Heavy Weapon | ✅ | |
| Iron Sight | ✅ | |
| Karnov's Revenge | ✅ | Built-in arcade emulator, with sound |
| Magical Drop 3 | ✅ | Built-in arcade emulator, with sound |
| Need for Speed - Carbon: Own the City | 🔄 | Career mode progresses since v0.4.2; performance remains low and some sections run in slow motion ([#36](https://github.com/ZeebxTeam/zeebx-emu/issues/36), [#81](https://github.com/ZeebxTeam/zeebx-emu/issues/81)) |
| Pac-Mania | ✅ | |
| Peggle | ✅ | |
| Powerboat Challenge | ✅ | Playable; menus, character and island selection; settings save persists |
| Prey 2 Evil | ✅ | Playable; menus, story, and level |
| Quake | 🔄 | Performance needs improvement; audio glitches |
| Quake 2 | 🔄 | Performance needs improvement |
| Raging Thunder 2 | ✅ | |
| Rally Master Pro | ✅ | Freeze after several races fixed in v0.4.3 |
| Reckless Racing | ✅ | |
| Resident Evil 4 - Zeebo Edition | ✅ | |
| Ridge Racer | ✅ | Playable; menus, race dashboard, and race. Freeze after the second lap fixed in v0.4.2 |
| Spin Master | ✅ | Built-in arcade emulator, with sound |
| Street Hoop | ✅ | Built-in arcade emulator, with sound |
| Super BurgerTime | ✅ | Built-in arcade emulator, with sound |
| Tekken 2 | ✅ | |
| Tork and Kral - A Prehistorik Adventure | ✅ | |
| Toy Raid | ✅ | |
| Brain Training (Treino Cerebral) | ✅ | |
| Monica's Gang: Let's Play, Vol. 1 | ✅ | Opens, navigates menus, reaches name selection |
| Ultimate Chess 3D | ✅ | |
| An Egg Game (Um Jogo de Ovos) | ✅ | Menus and controls respond |
| Wizard Fire | ✅ | Built-in arcade emulator, with sound |
| Zeebo Clube | ❌ | Depends on online system; recreation pending |
| Zeebo Extreme Baja | ✅ | |
| Zeebo Extreme Boia Cross | ✅ | |
| Zeebo Extreme Air Race | ✅ | |
| Zeebo Extreme Jetboard | ✅ | |
| Zeebo Extreme Rolimã | ✅ | |
| Zeebo Family Pack | ✅ | |
| Zeebo F.C. Foot Camp | ✅ | |
| Zeebo F.C. Super League | ✅ | Inherits Zeeboids online features |
| Zeeboids | ✅ | Online features about 50% functional |
| Zeebo Sports Peteca | ✅ | |
| Zeebo Sports Queimada | ✅ | |
| Zeebo Sports Tennis | ✅ | |
| Zeebo Sports Volleyball | ✅ | |
| Zeetris (Homebrew) | ✅ | |
| Zenonia | ✅ | On the AArch64 Libretro core, devices such as the R36S show a black screen although the game opens with sound ([#62](https://github.com/ZeebxTeam/zeebx-emu/issues/62)) |
| Zuma's Revenge! | ✅ | Playable |
| Z-Wheel | 🔄 | Usable, but still has many bugs; online features are not implemented |

Compatibility was measured with a particular build and ROM set. Results may differ on other frontends and devices; please report issues.
