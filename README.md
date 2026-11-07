# PeaceEmu-NES

Cœur d'émulation Nintendo Entertainment System (NES) écrit en Rust pour le projet [PeaceEmu](https://github.com/Likounsee/PeaceEmu).

- **Code du module :** [NES/](NES/)
- **État et limites :** [NES/README.md](NES/README.md)
- **Architecture :** [NES/docs/ARCHITECTURE.md](NES/docs/ARCHITECTURE.md)
- **Références et politique de réutilisation :** [NES/docs/REFERENCE_IMPLEMENTATIONS.md](NES/docs/REFERENCE_IMPLEMENTATIONS.md)
- **Tests :** [NES/tests/README.md](NES/tests/README.md)

## État du projet

Le dépôt contient un cœur Rust fonctionnel en cours de développement : CPU Ricoh 2A03, bus mémoire, cartouches et plusieurs mappers iNES, PPU, APU, contrôleurs, machine et API de session. Un exécutable de test Windows est également présent dans `NES/src/bin/peaceemu-nes-desktop.rs`.

**Avancement estimé du cœur : 73 %.** Il s'agit d'une estimation d'ingénierie, pas d'un pourcentage de jeux compatibles ni d'une couverture de tests. Les limites principales restent la précision cycle par cycle, le pipeline PPU par dot, l'arbitrage DMC, la validation avec des ROM de diagnostic et la preuve de l'intégration complète avec le dépôt PeaceEmu.

### Capacités actuelles (résumé)

- CPU 2A03 avec un ensemble étendu d'instructions officielles et non officielles courantes ; les opcodes inconnus sont rejetés explicitement.
- Bus avec RAM miroir, registres matériels, deux manettes série, DMA OAM et accès DMC initial.
- Parseur iNES legacy et implémentations locales de NROM (0), MMC1 (1), UxROM (2), CNROM (3), MMC3 (4), AxROM (7), MMC2 (9), MMC4 (10), Color Dreams (11), CPROM (13), Mapper 15, BNROM (34), GxROM (66), Camerica (71) et UNROM-512 (30). Les variantes de chaque carte ne sont pas toutes prises en charge.
- PPU avec registres, mémoire vidéo, timing de balayage et rendu 256×240 par ligne/snapshot ; ce n'est pas une pipeline pixel-par-pixel complète.
- L'évaluation des sprites est désactivée sur la pré-scanline 2C02 ; la première ligne visible commence sans sprites, puis l'évaluation dot-par-dot reprend sur les lignes visibles.
- L'évaluation OAM démarre maintenant à l'adresse exacte indiquée par OAMADDR au dot 65, y compris les démarrages au milieu d'un sprite et la copie qui traverse une frontière d'entrée OAM.
- Le compteur de sprites OAM est bien limité à six bits : il reboucle de l'entrée 63 à l'entrée 0 et ne termine l'évaluation qu'après avoir parcouru les 64 entrées depuis son point de départ.
- APU avec génération audio approximative, séquenceur de frame et premier DMC.
- Machine/session exposant frames, PCM, entrées et accès à la RAM batterie.

**NES 2.0 est pris en charge partiellement, pas de manière générale.** Des chemins explicites existent pour NROM, MMC1, UxROM, CNROM, MMC3, Mapper 8, GxROM et Camerica, avec des limites de tailles ROM/RAM et de sous-mappers propres à chaque carte. Les autres mappers et variantes non explicitement gérés sont rejetés ; ne pas annoncer une compatibilité NES 2.0 complète.

## Vérifications locales

Depuis la racine du dépôt :

```sh
cargo fmt --manifest-path NES/Cargo.toml -- --check
cargo test --manifest-path NES/Cargo.toml --locked
cargo clippy --manifest-path NES/Cargo.toml --all-targets --locked -- -D warnings
cargo build --manifest-path NES/Cargo.toml --bin peaceemu-nes-desktop --release --locked
```

Le dépôt contient des workflows de vérification dans `.github/workflows/`. Le statut doit être vérifié directement dans [GitHub Actions](https://github.com/Likounsee/PeaceEmu-NES/actions). La validation complète du 7 novembre 2026 (NES module CI, exécution `42419984382`) a réussi sur Windows et Linux : tests Rust, diagnostics Blargg CPU/PPU étendus, suites expérimentales APU/PPU/MMC3, tests DMA/dummy-write, Clippy et compilation Release. Une CI verte confirme ces vérifications automatisées, pas la compatibilité de tous les jeux.

## Version et publication

Les manifestes racine et `NES/Cargo.toml`, ainsi que `Cargo.lock`, sont alignés sur `1.0.0`. La validation technique de la version est terminée ; la publication GitHub reste déclenchée par le tag `nes-module-1.0.0`, qui construit le binaire Windows et son fichier SHA-256. Voir [RELEASE_NOTES_v1.0.0.md](RELEASE_NOTES_v1.0.0.md). Le projet conserve Rust 1.78 comme MSRV déclarée ; `NES/rust-toolchain.toml` utilise le canal `stable`.

## ROM et licences

Ne commitez aucune ROM commerciale ni aucun fichier de jeu dont la redistribution n'est pas autorisée. Utilisez des homebrews ou des suites de test dont la licence permet explicitement leur utilisation. Toute reprise de code tiers doit être documentée avec sa provenance, sa licence et les notices requises.
