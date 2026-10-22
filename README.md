# PeaceEmu-NES

Cœur d'émulation Nintendo Entertainment System pour l'application PeaceEmu.

Le développement se trouve dans [NES/](NES/). Consultez le [README du module](NES/README.md) et sa [documentation d'architecture](NES/docs/ARCHITECTURE.md).

## État du projet

**Avancement estimé : 30 %.** Le crate Rust comprend un CPU Ricoh 2A03 partiel, un bus RAM miroir, un parseur iNES/NROM mapper 0, un wrapper `NesMachine` et une PPU avec registres CPU, mémoire graphique, OAM, VBlank/NMI et un rendu de fond par instantané vers un framebuffer 256×240 d'indices de palette NES. Le DMA OAM copie 256 octets et ajoute 513/514 cycles de blocage.

**Ce n'est pas encore un émulateur NES jouable.** Le rendu actuel est un instantané de fond, pas un rendu synchronisé dot par dot ; sprites, hit sprite 0, APU, manettes, timing matériel précis, instructions CPU restantes et adaptateur au contrat EmulationCore restent à implémenter. La stratégie de partage/versionnement du crate de contrat hôte doit être décidée avant l'intégration compilable.

CI : vérifier le dernier workflow après les corrections de régressions DMA/NMI et l'ajout du rendu de fond.

## Développement local

Depuis la racine du dépôt :

~~~sh
cd NES
cargo fmt --all -- --check
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
~~~

Le workflow GitHub Actions dans .github/workflows/nes.yml exécute ces contrôles.

Aucune ROM commerciale ne doit être ajoutée au dépôt. Utiliser des programmes homebrew ou des suites de test dont la licence autorise l'usage.
