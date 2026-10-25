# PeaceEmu-NES

Cœur d'émulation Nintendo Entertainment System pour l'application PeaceEmu.

Le développement se trouve dans [NES/](NES/). Consultez le [README du module](NES/README.md) et la [documentation d'architecture](NES/docs/ARCHITECTURE.md).

## État réel du projet

**Avancement estimé : 79 %** (estimation du développement, pas un taux de compatibilité des jeux ; voir `NES/README.md` pour l'inventaire détaillé par domaine).

Le crate Rust contient un CPU Ricoh 2A03 couvrant un large sous-ensemble d'instructions officielles ainsi que plusieurs NOP non officiels fréquents, un bus avec RAM miroir, 8 Kio de PRG-RAM de cartouche, chargement des trainers iNES, les ports manette série, le DMA OAM et un accès DMC initial, le parsing iNES/NROM mapper 0, une machine de haut niveau, une PPU avec registres/mémoire graphique/VBlank/NMI et rendu snapshot 256×240, ainsi qu'une APU avec séquenceur de frame et synthèse audio approximative. Les interruptions NMI/IRQ, plusieurs règles de timing PPU et les compteurs de cycles DMA OAM sont couverts par des tests.

**Le cœur n'est pas encore déclaré jouable ni compatible avec une ROM complète.** La PPU rend encore une image par instantané plutôt que pixel par pixel. Le timing CPU/bus et le blocage DMC ne sont pas cycle-exacts ; les mappers autres que NROM, les filtres/resampling audio et l'adaptateur vers le contrat `EmulationCore` de PeaceEmu restent à faire. Aucun pourcentage ci-dessus ne doit être interprété comme une compatibilité commerciale.

Les tests de session couvrent aussi la normalisation du PCM (valeurs finies dans [-1, 1]) et les bornes inclusives de fréquence audio configurables (8 000–192 000 Hz). Le statut CI doit être confirmé sur le commit le plus récent avant de déclarer ces ajouts validés.

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
