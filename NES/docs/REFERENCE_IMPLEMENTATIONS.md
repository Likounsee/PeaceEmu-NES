# Références matérielles, implémentations et licences

## Objectif et règle de base

PeaceEmu-NES compare ses comportements à des spécifications matérielles et à des implémentations existantes. Les références servent à réduire les erreurs d'interprétation ; elles ne prouvent pas automatiquement la compatibilité et ne justifient pas d'importer un émulateur complet sans adaptation.

**Politique actuelle :** le code du cœur reste implémenté dans les structures de PeaceEmu-NES. Ce document ne revendique pas de copie ligne à ligne de code tiers. Si du code est effectivement repris à l'avenir, inscrire ici le fichier source exact, le commit/version, la licence, les modifications et les notices à conserver avant de fusionner ce portage.

## Références consultées

### Spécifications matérielles NESdev

- [NESdev Wiki](https://www.nesdev.org/wiki/Nesdev_Wiki) — référence principale pour le CPU 2A03, les registres PPU/APU, les mappers, les formats iNES/NES 2.0 et les timings.
- [MMC2](https://www.nesdev.org/wiki/MMC2) — banques PRG/CHR et latches de CHR.
- [UNROM 512](https://www.nesdev.org/wiki/UNROM512) — registre de banque, CHR-RAM et options de carte.

Les pages NESdev décrivent du matériel et des conventions ; vérifier la page spécifique et la révision de carte lorsqu'un comportement dépend du mapper.

### TetaNES

- Projet : [lukexor/tetanes](https://github.com/lukexor/tetanes)
- Cœur : [tetanes-core](https://github.com/lukexor/tetanes/tree/main/tetanes-core)
- Textes de licence annoncés par le dépôt : [MIT](https://github.com/lukexor/tetanes/blob/main/LICENSE-MIT) et [Apache-2.0](https://github.com/lukexor/tetanes/blob/main/LICENSE-APACHE).

Les tables d'opcodes et l'organisation des composants ont servi de points de comparaison pour certains comportements CPU/PPU/APU. La licence d'un projet ne s'applique pas automatiquement à une implémentation indépendante : si des lignes de code sont reprises, leur provenance et les obligations correspondantes doivent être documentées.

### RustyNES

- Projet : [doublegate/RustyNES](https://github.com/doublegate/RustyNES)
- Le dépôt est associé à la licence GPL-3.0.

**Ne pas reprendre de code RustyNES** sans décision explicite sur les conséquences de licence et la compatibilité avec la distribution de PeaceEmu. La simple lecture d'une référence n'autorise pas la copie de son code.

## Comportements déjà adaptés dans le cœur

La liste ci-dessous décrit des familles de comportements implémentées localement. Elle ne garantit pas la conformité à toutes les révisions matérielles.

| Zone | Adaptations présentes | Limites importantes |
|---|---|---|
| CPU | LAX/SAX, ANC/ALR/ARR/AXS, opérations composites SLO/RLA/SRE/RRA/DCP/ISC, NOP avec lectures, certains stores instables AHX/TAS/SHX/SHY | Pas d'exécution complète du bus à chaque cycle ; certains opcodes instables sont des approximations |
| Cartouches | NROM, MMC1, UxROM, CNROM, MMC3, MMC2, MMC4, AxROM, CPROM, GxROM, Color Dreams, Camerica, BNROM et UNROM-512, plus mapper 15 | Couverture limitée aux variantes et tailles explicitement prises en charge ; NES 2.0 global et tous les bus conflicts non pris en charge |
| PPU | Alias de palettes, mirroring nametable, progression de scanline/dot, NMI et certains événements de scroll | Pipeline de pixels/fetchs par dot incomplète ; IRQ MMC3 encore approximée |
| APU | Séquenceur de frame, enveloppes/sweep/compteur linéaire approximatifs, sortie PCM et DMC initial | Filtres, resampling hôte et arbitrage DMC cycle-exact non terminés |

Pour le détail des comportements actuels, consulter `NES/src/cpu.rs`, `NES/src/cartridge.rs`, `NES/src/ppu.rs` et `NES/src/apu.rs`, ainsi que les tests correspondants. Le code source reste la référence pour ce qui est réellement présent.

## Méthode pour un futur portage

1. **Définir un écart observable.** Ajouter un test qui reproduit le problème ou une attente matérielle non couverte.
2. **Lire la spécification.** Identifier le comportement et les exceptions de la puce ou de la révision de carte visée.
3. **Comparer une implémentation.** Vérifier son état, sa licence, son historique et les tests pertinents.
4. **Choisir une stratégie.** Réécriture selon le comportement documenté ou portage explicite d'un petit composant sous licence compatible.
5. **Conserver la provenance.** Pour un portage réel : URL du fichier, commit/version, licence, changements locaux et notices requises.
6. **Adapter à l'architecture locale.** Garder les API `CpuBus`, `Cartridge`, `Ppu`, `Apu`, `NesMachine` et `NesSession` cohérentes ; ne pas importer le frontend d'un autre émulateur.
7. **Ajouter des tests de régression.** Couvrir cas nominal, limites, timing et erreurs.
8. **Vérifier la CI.** Formatage strict, tests, Clippy et build des cibles concernées.

## Politique de dépendances

Ne pas ajouter une dépendance d'émulateur complète comme raccourci sans examiner :
- licence et notices ;
- MSRV réelle et dépendances transitives ;
- modèle de bus, mappers et ordonnanceur ;
- API vidéo/audio et impact sur le contrat hôte ;
- possibilité de tester le comportement sans importer un frontend.

Le dépôt contient actuellement deux valeurs différentes de `rust-version` entre le manifeste racine et `NES/Cargo.toml`. Ne pas documenter une MSRV unique tant que cette contradiction n'est pas résolue.

## Règles de publication

- Ne pas inclure de ROM commerciale.
- Ne pas revendiquer la compatibilité d'un jeu sans test reproductible.
- Ne pas confondre code réécrit selon une spécification et code repris d'un projet tiers.
- En cas de reprise de code, vérifier la licence du commit/fichier exact, conserver les notices requises et obtenir une décision de licence lorsque nécessaire.
