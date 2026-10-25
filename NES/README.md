# PeaceEmu-NES — cœur d'émulation NES

Cœur matériel Nintendo Entertainment System en Rust. Le but est de fournir à [PeaceEmu](https://github.com/Likounsee/PeaceEmu) une machine NES testable et indépendante de l'interface graphique.

## État en un coup d'œil

**Avancement estimé : 70 %.** Cette estimation porte sur le travail d'ingénierie du cœur ; elle ne mesure ni la couverture des lignes, ni le taux de jeux compatibles.

| Domaine | Estimation | Résumé |
|---|---:|---|
| CPU Ricoh 2A03 | 74 % | Instructions officielles courantes, opcodes non officiels sélectionnés, interruptions ; timing incomplet |
| PPU | 72 % | Registres, mémoire vidéo, VBlank/NMI, rendu 256×240 ; pipeline par dot incomplète |
| APU | 64 % | Pulse/triangle/bruit, séquenceur et DMC initial ; mixage et DMA approximatifs |
| Bus et manettes | 76 % | RAM miroir, registres, ports série, DMA OAM, premiers accès DMC |
| Cartouches/mappers | 86 % | Nombreuses cartes iNES legacy ; variantes et NES 2.0 non générales |
| Machine/session | 77 % | Chargement, reset, frames, PCM, entrées et RAM batterie |
| Tests et robustesse | 94 % | Régressions unitaires et d'intégration ; validation ROM réelle à élargir |
| Intégration hôte | 45 % | L'adaptateur et le parcours complet doivent être validés avec le dépôt principal |

Les valeurs par domaine sont des estimations indicatives ; elles ne doivent pas être moyennées comme une métrique de compatibilité.

## Dernière itération

- **APU/DMC** : tests de propriété du sample buffer, de livraison DMA sans double consommation et de séparation des IRQ frame/DMC, y compris lors de l'inhibition immédiate via `$4017`.
- **MMC3** : test complémentaire garantissant qu'un niveau A12 maintenu à 1 ne redéclenche pas l'IRQ ; un nouveau déclenchement exige un retour bas suffisamment long puis un vrai front montant qualifié.
- **Séquenceur APU** : tests du délai de reset de `$4017` selon la parité CPU (3/4 cycles), de l'inhibition immédiate de l'IRQ frame et du comportement de lecture de statut.
- **Bus/machine** : test du fetch DMC depuis la ROM via le bus CPU et des cycles de blocage qui font progresser l'horloge PPU partagée.
- **PPU/NMI/scroll** : régressions pour la suppression de VBlank par `$2002`, l'activation de NMI pendant VBlank et les retours de coarse-Y (ligne 29 : changement de nametable ; ligne 31 : pas de changement).
- **MMC3** : tests du filtre A12 à sept/huit dots, du rebasage après reset PPU, de l'acquittement/réactivation IRQ, du latch non nul, de `$C001` et du maintien du rechargement lorsqu'un front est filtré.
- Une fixture MMC3 utilise des octets de signature iNES explicites pour éviter toute ambiguïté d'échappement dans le test.
- Les résultats finaux des workflows doivent être vérifiés après les derniers commits ; ces tests synthétiques ne remplacent pas une validation avec des ROM de diagnostic et des essais de jeu réels.

## Ce qui est implémenté

- **CPU :** interpréteur 2A03 avec de nombreuses instructions officielles, plusieurs opcodes non officiels stables et composites, flags, pile et interruptions. Les opcodes inconnus produisent une erreur explicite. Le modèle n'est pas cycle-exact.
- **Bus :** RAM interne de 2 Kio et miroirs, espace cartouche, registres PPU/APU, ports série des manettes, DMA OAM à blocage nominal de 513/514 cycles et accès DMC initial.
- **Cartouches :** parseur iNES legacy, trainers, PRG-RAM/CHR-RAM, mirroring et dispatch statique pour NROM (0), MMC1 (1), UxROM (2), CNROM (3), MMC3 (4), AxROM (7), Front Fareast F3xxx (8), MMC2 (9), MMC4 (10), Color Dreams (11), CPROM (13), Mapper 15, BNROM (34), GxROM (66), Camerica (71), UNROM-512 (30) et Jaleco JF-16 (87). UxROM/CNROM gèrent aussi les bus conflicts quand le sous-mapper NES 2.0 indique explicitement cette variante.
- **PPU :** registres CPU, VRAM, nametables, palettes, OAM, sprites, scroll, VBlank/NMI et progression du balayage. Le rendu visible progresse au niveau des dots pour certaines opérations, avec composition sprite par pixel et sprite-zero hit au point de recouvrement ; l'évaluation matérielle complète des sprites et tous les fetchs ne sont toutefois pas encore reproduits.
- **APU :** pulse, triangle, bruit, séquenceur de frame, sortie PCM mono et premier DMC avec buffer, unité delta, bouclage et IRQ. Le timing du DMC et le resampling hôte sont à améliorer.
- **Machine et session :** chargement/reset de ROM, exécution de frames, framebuffer, audio PCM, entrées manette et accès à la RAM batterie.
- **Frontend de développement :** un binaire Windows autonome existe pour les essais. Il ne doit pas être confondu avec une intégration confirmée dans l'interface PeaceEmu.

## Limites connues

1. CPU, PPU, APU et bus ne reproduisent pas tous les accès à l'échelle du cycle matériel.
2. Le pipeline PPU par dot, les fetchs et les événements MMC3 basés sur les transitions A12 filtrées restent à finaliser.
3. L'arbitrage DMA DMC, les filtres audio, le resampling à la fréquence hôte et certains détails de séquenceur APU sont approximatifs.
4. Les mappers implémentés ne couvrent pas toutes les révisions de cartes ni tous les sous-mappers. Les bus conflicts ne sont modélisés que pour les sous-mappers NES 2.0 explicitement pris en charge de UxROM/CNROM ; les variantes iNES historiques restent indéterminées.
5. NES 2.0 est pris en charge partiellement pour NROM, UxROM, CNROM, Mapper 8, GxROM (66) et Camerica (71), notamment le décodage de tailles ROM et de certaines tailles RAM. Pour Camerica, seuls les sous-mappers 0, les tailles PRG compatibles et jusqu'à 8 Kio de PRG-RAM sont acceptés ; les autres variantes restent refusées. Les autres mappers et sous-mappers non explicitement gérés restent refusés ou non validés.
6. La suite synthétique couvre maintenant des cas supplémentaires de sprite-zero hit, masquage du bord gauche, priorité OAM et coordonnées Y wrap-around, ainsi que les tailles RAM/trainer du mapper 71. Elle doit être complétée par des ROM de diagnostic/homebrew redistribuables légalement.
7. Le contrat `EmulationCore`, l'adaptateur et le lancement depuis la bibliothèque PeaceEmu doivent être compilés et testés ensemble dans le dépôt hôte.
8. Les deux manifestes `Cargo.toml` déclarent désormais Rust 1.78 comme MSRV ; il reste à vérifier cette promesse avec une toolchain 1.78 en CI.

## Intégration PeaceEmu

Le cœur ne doit pas dépendre de egui/eframe ni recréer la bibliothèque de jeux. Le dépôt hôte est responsable du cycle de vie, de la présentation vidéo/audio, de la collecte des entrées et des sauvegardes persistantes.

Le contrat partagé `EmulationCore` réside dans le dépôt principal. Avant de revendiquer une intégration reproductible, choisir une stratégie de dépendance/versionnement pour ce crate, puis vérifier le parcours complet : découverte de ROM, chargement, reset, vidéo, audio, manettes et RAM batterie. Les save states ne doivent pas être annoncés comme pris en charge sans implémentation et tests.

## Développement local

Depuis la racine du dépôt :

```sh
cargo fmt --manifest-path NES/Cargo.toml -- --check
cargo test --manifest-path NES/Cargo.toml --locked
cargo clippy --manifest-path NES/Cargo.toml --all-targets --locked -- -D warnings
cargo build --manifest-path NES/Cargo.toml --bin peaceemu-nes-desktop --release --locked
```

Consultez [Architecture](docs/ARCHITECTURE.md), [Références et réutilisation](docs/REFERENCE_IMPLEMENTATIONS.md) et [Stratégie de tests](tests/README.md). Vérifiez toujours le résultat du workflow sur le commit concerné.

## Feuille de route priorisée

### P0 — fiabilité du dépôt
- Aligner les README, les manifestes Rust et les capacités réellement exposées.
- Faire de la CI un contrôle strict du formatage (`cargo fmt --check`), des tests et de Clippy.
- Ajouter des tests de parsing négatifs, de débordements/tailles, et des cas limites de sauvegarde.

### P1 — validation fonctionnelle
- Ajouter des ROM de diagnostic redistribuables légalement et conserver les résultats attendus.
- Vérifier les instructions CPU, interruptions, DMA, PPUSTATUS/NMI, IRQ APU et comportement des mappers.
- Démontrer le chargement et une session réelle depuis le dépôt PeaceEmu.

### P2 — précision matérielle
- Construire progressivement le pipeline PPU par dot et les fetchs de tuiles.
- Exposer les accès PPU nécessaires aux IRQ MMC3 et gérer les transitions A12 avec le filtrage approprié.
- Raffiner l'arbitrage DMC, les cycles de blocage et la sortie audio.

### P3 — extension de compatibilité
- Documenter les révisions et variantes prises en charge par mapper.
- Étendre NES 2.0 de manière structurée, avec tailles de RAM et sous-mappers explicites.
- Ajouter les autres mappers uniquement avec tests et limites documentées.

## Règles du projet

- Ne pas déclarer un jeu compatible sur la seule base d'une compilation.
- Ne pas supprimer une régression pour faire passer la CI.
- Ne pas ajouter de ROM commerciale ni de contenu dont la redistribution n'est pas autorisée.
- Ne pas reprendre de code tiers sans provenance, licence et notices vérifiées.
