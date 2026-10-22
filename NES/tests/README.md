# Tests NES

## Tests présents

Les tests se trouvent près des modules dans les blocs cfg(test) de la bibliothèque Rust.

- **CPU :** LDA/ADC/STA, flags zéro/carry/négatif/overflow, branchement relatif, JSR/RTS, coût nominal des cycles et rejet explicite d'un opcode inconnu.
- **Bus :** miroir de la RAM interne sur les plages 0x0000–0x1FFF, latch open-bus simplifié et insertion d'une cartouche.
- **Cartouches :** signature iNES, en-tête tronqué, mapper non pris en charge, tailles PRG invalides, image tronquée, NES 2.0 non pris en charge, mirroring, CHR-RAM et mirroring de PRG pour une ROM de 16 Kio.

## Couverture à ajouter

- **CPU :** chaque opcode officiel et chaque mode d'adressage, calcul exact des cycles, pénalités de franchissement de page, interruptions IRQ/NMI, cas limites des flags et tests de référence.
- **Bus :** registres PPU/APU, manettes, open bus matériel et interaction cartouche/bus.
- **Cartouches :** bits de mirroring, trainer, tailles extrêmes, PRG-RAM, validation plus stricte et tests de mapper.
- **PPU/APU :** registres, timing, rendu, interruptions et sortie déterministe.
- **Intégration :** démarrage avec ROM homebrew redistribuable et suites de validation dont la licence autorise l'utilisation.

Les tests doivent être reproductibles et automatisés en CI. Compiler n'est pas une preuve de compatibilité. Ne pas supprimer une régression pour faire passer la CI ; documenter les limites connues.
