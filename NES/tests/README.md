# Tests et validation du cœur NES

## Objectif

La suite protège les comportements du cœur et évite les régressions. Elle ne constitue pas, à elle seule, une certification de compatibilité avec les jeux commerciaux. Les tests doivent rester déterministes, reproductibles et exécutables en CI.

## Où sont les tests ?

- **Tests unitaires :** dans `NES/src/*.rs`, à l'intérieur des modules.
- **Tests d'intégration :** dans `NES/tests/`.
- `mapper30_banking.rs` — banques PRG/CHR-RAM et mirroring de la variante UNROM-512 ciblée.
- `mapper30_boundaries.rs` — limites de taille, trainer et rejet des images non prises en charge.
- `ppu_nmi_timing.rs` — événements VBlank/NMI via l'API publique de la PPU.

## Couverture déjà présente (vue fonctionnelle)

- **CPU :** instructions représentatives, flags, branchements, pile, opérations composites et certains opcodes non officiels, cycles nominaux, rejet des opcodes inconnus et cas de franchissement de page.
- **Bus :** miroirs de RAM, accès cartouche, manettes série, DMA OAM et interactions matérielles ciblées.
- **Cartouches :** signatures iNES, en-têtes et images tronqués, mappers inconnus, tailles invalides, trainer, mirroring, PRG-RAM/CHR-RAM, sélection de banques et erreurs de parsing.
- **PPU :** registres, buffer PPUDATA, palettes et alias, mirroring nametable, scroll, sprites, VBlank/NMI, progression des dots/scanlines et saut de dot des images impaires.
- **APU :** registres, séquenceur de frame, compteurs de longueur, IRQ, sortie PCM normalisée, limites de fréquence audio, DMC et transferts d'échantillons.
- **Machine/session :** reset et chargement, exécution de frames, framebuffer, PCM et accès à la RAM batterie.

La liste est descriptive et non exhaustive. Vérifier le code des tests avant de supposer qu'un scénario précis est couvert.

## Priorités de validation

### P0 — contrôle reproductible
- Exécuter `cargo fmt --all -- --check`, `cargo test --all-targets --locked` et `cargo clippy --all-targets --locked -- -D warnings`.
- Conserver un workflow qui échoue si le formatage versionné est incorrect ; ne pas lancer simplement `cargo fmt` en CI.
- Tester sur Windows et Linux, en distinguant compilation du binaire et exécution interactive.

### P1 — tests matériels de référence
- CPU : tous les opcodes supportés, flags, cycles, interruptions, accès factices et cas limites d'adressage.
- PPU : fetchs par dot, événements de scroll, hit/overflow sprite, accès concurrents et interactions NMI/PPUSTATUS.
- APU/bus : timing IRQ, séquenceur, DMC, alignement et arbitrage DMA.
- Mappers : tailles, banques extrêmes, mirroring, RAM, bus conflicts et variantes explicitement ciblées.

### P2 — ROM de diagnostic
Ajouter uniquement des ROM dont la licence autorise l'intégration ou le téléchargement automatisé. Pour chaque ROM, documenter :
- la source et la licence ;
- la commande exacte pour lancer le test ;
- le résultat attendu et le résultat observé ;
- la version du cœur et la plateforme ;
- les échecs connus, sans masquer les régressions.

Ne pas commiter de ROM commerciale. Si une ROM de diagnostic ne peut pas être redistribuée, documenter une méthode d'obtention licite et permettre de fournir le fichier localement.

### P3 — validation de bout en bout
Tester le parcours depuis le dépôt principal PeaceEmu : découverte de la ROM, chargement, reset, plusieurs frames, affichage, audio, entrées et sauvegarde batterie. Une compilation du crate NES ou une CI verte ne prouve pas que ce parcours fonctionne.

## Commandes locales

Depuis la racine du dépôt :

```sh
cargo fmt --manifest-path NES/Cargo.toml -- --check
cargo test --manifest-path NES/Cargo.toml --locked
cargo clippy --manifest-path NES/Cargo.toml --all-targets --locked -- -D warnings
cargo build --manifest-path NES/Cargo.toml --bin peaceemu-nes-desktop --release --locked
```

Si l'une des commandes échoue, corriger la cause et conserver le test de régression approprié. Ne pas supprimer ou affaiblir un test uniquement pour faire passer la CI.
