# Tests NES

## Tests présents

Les tests se trouvent près des modules dans les blocs cfg(test) de la bibliothèque Rust.

- **CPU :** LDA/ADC/STA, flags zéro/carry/négatif/overflow, branchement relatif, JSR/RTS, coût nominal des cycles et rejet explicite d'un opcode inconnu.
- **Bus :** miroir de la RAM interne sur les plages 0x0000–0x1FFF, latch open-bus simplifié et insertion d'une cartouche.
- **Cartouches :** signature iNES, en-tête tronqué, mapper non pris en charge, tailles PRG invalides, image tronquée, NES 2.0 non pris en charge, mirroring, CHR-RAM, mirroring de PRG pour une ROM de 16 Kio et instantanés PRG-RAM par mapper.
- **Sauvegardes :** API de RAM batterie sur `NesMachine`, aller-retour de 8 Kio, refus si le flag batterie iNES est absent et rejet atomique d'une taille incorrecte.

## Couverture à ajouter

- **CPU :** chaque opcode officiel et chaque mode d'adressage, calcul exact des cycles, pénalités de franchissement de page, interruptions IRQ/NMI, cas limites des flags et tests de référence.
- **Bus :** registres PPU/APU, manettes, open bus matériel et interaction cartouche/bus.
- **Cartouches :** bits de mirroring, trainer, tailles extrêmes, distinction entre RAM volatile et batterie, tailles de RAM spécifiques aux cartouches et validation plus stricte.
- **PPU :** registres, vblank/NMI, rendu snapshot, priorité des sprites et rollover vertical coarse-Y (ligne 29 bascule le nametable, ligne 31 non).
- **APU :** registres, timing, interruptions et sortie audio déterministe approximative.
- **PPU vblank/NMI :** activation tardive du NMI, acquittement via PPUSTATUS et transitions du signal NMI (NES/tests/ppu_nmi_timing.rs).
- **Mapper 30 :** limites du trainer/image (mapper30_boundaries.rs), banques PRG/CHR-RAM, mirroring One-Screen et mode quatre écrans (mapper30_banking.rs).
- **Intégration :** démarrage avec ROM homebrew redistribuable et suites de validation dont la licence autorise l'utilisation.

Les tests doivent être reproductibles et automatisés en CI. Compiler n'est pas une preuve de compatibilité. Ne pas supprimer une régression pour faire passer la CI ; documenter les limites connues.
