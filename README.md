# PeaceEmu-NES

Cœur d'émulation Nintendo Entertainment System (NES) écrit en Rust pour le projet [PeaceEmu](https://github.com/Likounsee/PeaceEmu).

- **Code du module :** [NES/](NES/)
- **État et limites :** [NES/README.md](NES/README.md)
- **Architecture :** [NES/docs/ARCHITECTURE.md](NES/docs/ARCHITECTURE.md)
- **Références et politique de réutilisation :** [NES/docs/REFERENCE_IMPLEMENTATIONS.md](NES/docs/REFERENCE_IMPLEMENTATIONS.md)
- **Tests :** [NES/tests/README.md](NES/tests/README.md)

## État du projet

Le dépôt contient un cœur Rust fonctionnel en cours de développement : CPU Ricoh 2A03, bus mémoire, cartouches et plusieurs mappers iNES, PPU, APU, contrôleurs, machine et API de session. Un exécutable de test Windows est également présent dans `NES/src/bin/peaceemu-nes-desktop.rs`.

**Avancement estimé du cœur : 65 %.** Il s'agit d'une estimation d'ingénierie, pas d'un pourcentage de jeux compatibles ni d'une couverture de tests. Les limites principales restent la précision cycle par cycle, le pipeline PPU par dot, l'arbitrage DMC, la validation avec des ROM de diagnostic et la preuve de l'intégration complète avec le dépôt PeaceEmu.

### Capacités actuelles (résumé)

- CPU 2A03 avec un ensemble étendu d'instructions officielles et non officielles courantes ; les opcodes inconnus sont rejetés explicitement.
- Bus avec RAM miroir, registres matériels, deux manettes série, DMA OAM et accès DMC initial.
- Parseur iNES legacy et implémentations locales de NROM (0), MMC1 (1), UxROM (2), CNROM (3), MMC3 (4), AxROM (7), MMC2 (9), MMC4 (10), Color Dreams (11), CPROM (13), Mapper 15, BNROM (34), GxROM (66), Camerica (71) et UNROM-512 (30). Les variantes de chaque carte ne sont pas toutes prises en charge.
- PPU avec registres, mémoire vidéo, timing de balayage et rendu 256×240 par ligne/snapshot ; ce n'est pas une pipeline pixel-par-pixel complète.
- APU avec génération audio approximative, séquenceur de frame et premier DMC.
- Machine/session exposant frames, PCM, entrées et accès à la RAM batterie.

**NES 2.0 n'est pas pris en charge de manière générale.** Le chemin NROM contient un décodage limité de certaines tailles NES 2.0, mais les autres implémentations de cartouches rejettent ce format. Ne pas annoncer une compatibilité NES 2.0 complète.

## Vérifications locales

Depuis la racine du dépôt :

```sh
cargo fmt --manifest-path NES/Cargo.toml -- --check
cargo test --manifest-path NES/Cargo.toml --locked
cargo clippy --manifest-path NES/Cargo.toml --all-targets --locked -- -D warnings
cargo build --manifest-path NES/Cargo.toml --bin peaceemu-nes-desktop --release --locked
```

Le dépôt contient deux workflows principaux de vérification : `.github/workflows/ci.yml` et `.github/workflows/nes.yml`. Le workflow de module le plus récent était vert lors de la dernière vérification ; consultez [GitHub Actions](https://github.com/Likounsee/PeaceEmu-NES/actions) pour le statut actuel. La CI verte confirme les vérifications automatisées, pas la compatibilité de tous les jeux.

## Note sur la version Rust

Les manifestes racine et `NES/Cargo.toml` annoncent actuellement des versions minimales différentes. Ce point doit être harmonisé avant de publier une politique MSRV. Le fichier `NES/rust-toolchain.toml` utilise le canal `stable`.

## ROM et licences

Ne commitez aucune ROM commerciale ni aucun fichier de jeu dont la redistribution n'est pas autorisée. Utilisez des homebrews ou des suites de test dont la licence permet explicitement leur utilisation. Toute reprise de code tiers doit être documentée avec sa provenance, sa licence et les notices requises.
