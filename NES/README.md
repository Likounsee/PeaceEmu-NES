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

- **CPU/bus** : les instructions implicites de deux cycles (transferts, flags, shifts accumulateur, NOPs implicites et KIL) effectuent maintenant leur lecture factice pendant le bon ordre de bus, plutôt que de laisser ce cycle à la fin de l'instruction. Tests ajoutés pour toutes les opcodes concernées.
- **Bus/APU** : test d'intégration de la lecture de `$4015` : bit 5 conservé depuis l'open bus et IRQ frame acquittée à la lecture.
- **Machine/DMA OAM** : test du chemin pair (513 cycles) et vérification du transfert des 256 octets ; le test existant couvre déjà le chemin impair (514 cycles).

- **PPU/PPUDATA** : test supplémentaire de l'alias palette `$3F10` → `$3F00` et du rechargement du buffer différé depuis l'adresse nametable correspondante `$2F10`.
- **MMC3/PRG-RAM** : le test d'activation/protection démarre désormais avec un état explicite (RAM activée et inscriptible), puis vérifie désactivation, protection et réactivation sans dépendre implicitement des valeurs de démarrage.
- **APU/DMC** : les tests couvrent le buffer d'échantillon, les stalls DMA, l'indépendance entre IRQ frame/DMC/mapper et la lecture DMC à travers une banque PRG MMC3 sélectionnée.
- **Session audio** : la documentation précise désormais que changer réellement de fréquence réinitialise la file PCM, tandis que réappliquer la fréquence courante est un no-op ; un test vérifie le contrat de configuration côté hôte.
- **PPU/sprite-zero** : les tests couvrent la limite du dernier pixel visible (hit autorisé à X=254, interdit à X=255), l'indépendance vis-à-vis de la priorité visuelle et l'alignement du hit avec le retournement horizontal du motif du sprite.
- **PPU/NMI/scroll** : régressions pour l'activation de NMI en plein VBlank (un seul front montant), le passage de coarse-Y 30 à 31 sans bascule de nametable et les limites du scroll vertical.
- **MMC3/CHR** : test exhaustif des huit fenêtres de 1 Kio en mode CHR normal et inversé, pour vérifier que les registres R0-R5 restent correctement placés sans alias.
- **MMC3/PRG** : nouvelle régression vérifiant les quatre fenêtres de 8 Kio en modes PRG 0 et 1, notamment le déplacement du registre R6 et le maintien des deux banques fixes.
- **PPU/sprites 8×16** : nouveaux tests des adresses de motifs aux frontières des deux tuiles et du retournement vertical sur les lignes 0, 7, 8 et 15 ; le mode 8×8 est aussi vérifié.
- **APU/DMC** : tests ajoutés pour le wrap d'adresse de `$FFFF` vers `$8000` après consommation des octets et pour le redémarrage de la lecture en mode loop sans écraser l'octet tamponné.
- **PPU/sprites 8×16** : test de rendu de pixels réels à travers la frontière entre les deux tuiles, avec retournement vertical ; la coordonnée de scanline tient compte du décalage Y de l'OAM.
- **MMC3/PRG-RAM** : test des trois états de la RAM ($A001) : lecture/écriture autorisées, RAM lisible mais protégée en écriture, puis RAM désactivée et réactivée sans fuite d'écritures.
- **Validation** : après ajustement du test de rendu 8×16 à la coordonnée Y de l'OAM et suppression d'une opération identité détectée par Clippy, la CI passe sur Rust stable, Rust 1.78 et Windows : tests, formatage et Clippy réussis. Les tests synthétiques ne remplacent pas les essais de ROM réelles, notamment pour l'alignement sprite/hitbox de *Super Mario Bros.*
- **APU/DMC IRQ** : nouvelle régression pour la levée de l'IRQ à la fin d'un échantillon d'un octet, sa lecture via `$4015` sans acquittement et son effacement via `$4010`.
- **MMC3/mirroring** : nouvelle régression garantissant que le registre `$A000` ne remplace pas le câblage four-screen imposé par la cartouche.
- **PPU/mémoire vidéo** : tests d'intégration des quatre nametables physiques en four-screen, de l'alias `$3000` → `$2000` et des alias de mirroring horizontal/vertical.
- **MMC3/trainer** : test du chargement du trainer aux adresses `$7000-$71FF` et de la conservation des données lorsque la PRG-RAM est désactivée puis réactivée.


## Ce qui est implémenté

- **Rendu des sprites pendant le balayage PPU** : le chemin dot-par-dot compose maintenant le pixel sprite au moment où le faisceau atteint sa coordonnée, avec priorité OAM, transparence, masquage des huit premiers pixels et priorité arrière-plan/sprite. Cela corrige une incohérence architecturale du chemin de rendu live qui produisait les pixels d'arrière-plan sans composer les sprites avant un rendu snapshot séparé. Trois tests ciblent l'apparition au bon dot, la priorité derrière un pixel de fond opaque et le passage d'un sprite transparent au suivant. Ce changement ne rend pas encore le pipeline 2C02 cycle-exact et ne prouve pas à lui seul la compatibilité SMB.


- **PPU/scroll en cours de scanline** : trois tests de régression isolent l'écriture du second octet de `$2005` (mise à jour de `t` sans changement immédiat du scroll actif), l'écriture complète de `$2006` pendant les dots visibles (nouvelle origine à la coordonnée du faisceau) et une écriture après la zone visible. Ils protègent le modèle de rendu incrémental, sans prétendre simuler tous les fetchs du 2C02 ni démontrer une résolution du décalage sprites/hitboxes de Super Mario Bros.


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
