# PeaceEmu-NES

Cœur d'émulation Nintendo Entertainment System pour l'application PeaceEmu.

Le développement se trouve dans [NES/](NES/). Consultez le [README du module](NES/README.md) et sa [documentation d'architecture](NES/docs/ARCHITECTURE.md).

## État du projet

**Avancement estimé : 28 %.** Le crate Rust comprend un CPU Ricoh 2A03 partiel, un bus RAM miroir, un parseur iNES/NROM mapper 0, un wrapper `NesMachine` et une première PPU avec registres CPU, mémoire graphique, OAM et timing VBlank, ainsi que le DMA OAM (256 octets et 513/514 cycles de blocage) et la génération des requêtes NMI en VBlank avec entrée d'interruption CPU.

**Ce n'est pas encore un émulateur NES jouable.** Le rendu PPU, l'APU, les manettes et le timing matériel précis, les instructions CPU restantes et l'adaptateur au contrat EmulationCore de PeaceEmu restent à implémenter. La stratégie de partage/versionnement du crate de contrat hôte doit être décidée avant l'intégration compilable.

CI après les changements NMI : en cours de vérification.

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
