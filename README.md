# PeaceEmu-NES

Cœur d'émulation Nintendo Entertainment System pour l'application PeaceEmu.

Le développement se trouve dans [NES/](NES/). Consultez le [README du module](NES/README.md) et sa [documentation d'architecture](NES/docs/ARCHITECTURE.md).

## État du projet

Le crate Rust initial est en place. Il comprend un premier interpréteur CPU Ricoh 2A03 pour un sous-ensemble d'instructions officielles, un bus avec RAM interne miroir et un parseur iNES limité au mapper NROM (0). Des tests unitaires accompagnent ces composants.

**Ce n'est pas encore un émulateur NES jouable.** La PPU, l'APU, les manettes, le timing matériel complet, la majorité des instructions CPU et l'adaptateur au contrat EmulationCore de PeaceEmu restent à implémenter. La stratégie de partage/versionnement du crate de contrat hôte doit être décidée avant l'intégration compilable.

## Développement local

Depuis la racine du dépôt :

~~~sh
cd NES
cargo fmt --all -- --check
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
~~~

Le workflow GitHub Actions dans .github/workflows/nes.yml exécute ces contrôles. Le statut CI doit être vérifié dans GitHub Actions avant d'être déclaré vert.

Aucune ROM commerciale ne doit être ajoutée au dépôt. Utiliser des programmes homebrew ou des suites de test dont la licence autorise l'usage.
