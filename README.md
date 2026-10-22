# PeaceEmu-NES

Cœur d'émulation Nintendo Entertainment System pour l'application PeaceEmu.

Le développement se trouve dans [NES/](NES/). Consultez le [README du module](NES/README.md) et la [documentation d'architecture](NES/docs/ARCHITECTURE.md).

## État réel du projet

**Avancement estimé : 57 %** (estimation du développement, pas un taux de compatibilité des jeux).

Le crate Rust contient un CPU Ricoh 2A03 couvrant un large sous-ensemble d'instructions officielles, un bus avec RAM miroir, les ports manette série, le DMA OAM et un accès DMC initial, le parsing iNES/NROM mapper 0, une machine de haut niveau, une PPU avec registres/mémoire graphique/VBlank/NMI et rendu snapshot 256×240, ainsi qu'une APU avec séquenceur de frame et synthèse audio approximative. Les interruptions NMI/IRQ, plusieurs règles de timing PPU et les compteurs de cycles DMA OAM sont couverts par des tests.

**Le cœur n'est pas encore déclaré jouable ni compatible avec une ROM complète.** La PPU rend encore une image par instantané plutôt que pixel par pixel. Le timing CPU/bus et le blocage DMC ne sont pas cycle-exacts ; les mappers autres que NROM, les filtres/resampling audio et l'adaptateur vers le contrat `EmulationCore` de PeaceEmu restent à faire. Aucun pourcentage ci-dessus ne doit être interprété comme une compatibilité commerciale.

La dernière CI validée exécute **99 tests** sur le commit documenté, avec formatage, tests et Clippy : [workflow GitHub Actions](https://github.com/Likounsee/PeaceEmu-NES/actions/runs/39845707220). Le commit le plus récent peut avoir une exécution CI encore en cours ; vérifier son statut avant de le déclarer validé.

## Développement local

Depuis la racine du dépôt :

~~~sh
cd NES
cargo fmt --all -- --check
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
~~~

Le workflow GitHub Actions dans `.github/workflows/nes.yml` exécute ces contrôles.

Aucune ROM commerciale ne doit être ajoutée au dépôt. Utiliser des homebrews ou des suites de test dont la licence autorise l'usage.
