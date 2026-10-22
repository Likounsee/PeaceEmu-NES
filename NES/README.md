# PeaceEmu-NES — module NES

> Module d'émulation de la Nintendo Entertainment System (NES) pour l'écosystème PeaceEmu.

## Objectif

Construire progressivement un émulateur NES modulaire, testable et documenté. La NES est un bon point de départ pour apprendre l'émulation : son architecture est plus petite que celle de consoles plus récentes, mais une émulation fidèle demande tout de même de respecter les cycles, les interruptions, le rendu vidéo et les particularités des cartouches.

**État actuel :** initialisation de la structure et de la documentation. Aucun cœur d'émulation fonctionnel n'est encore implémenté.

## Périmètre

- CPU Ricoh 2A03 (dérivé du 6502) : instructions, flags, adressage, cycles et interruptions.
- Bus mémoire : RAM interne, registres mappés en mémoire et accès aux composants.
- PPU : registres, mémoire vidéo, sprites, arrière-plan et timing.
- APU : canaux audio et synchronisation.
- Cartouches : parsing iNES/NES 2.0, PRG/CHR et premiers mapper(s).
- Entrées : contrôleurs standard.
- Machine : horloge commune et synchronisation CPU/PPU/APU.
- Frontend : chargement de ROM, affichage, audio et commandes utilisateur, séparés du cœur.
- Tests : tests unitaires, ROM de validation légales et tests de non-régression.

## Architecture visée

Les composants doivent rester indépendants et communiquer via des interfaces explicites :

```text
NES/
├── README.md
├── docs/
│   └── ARCHITECTURE.md
├── src/
│   └── README.md
└── tests/
    └── README.md
```

Cette structure est volontairement neutre vis-à-vis du langage : le choix du langage et du système de build doit être confirmé avant d'ajouter le cœur exécutable.

## Ordre de développement

1. **Fondations** — conventions, langage/build, types, logs et tests.
2. **CPU** — registres, modes d'adressage et instructions, puis tests unitaires.
3. **Bus et RAM** — lecture/écriture et décodage des adresses.
4. **ROM et cartouches** — validation d'en-tête iNES et NROM (mapper 0).
5. **PPU minimale** — registres et rendu de base, puis synchronisation.
6. **Boucle machine** — exécution coordonnée et interruptions.
7. **Contrôleurs et APU** — entrées puis audio.
8. **Compatibilité** — suites de tests, jeux homebrew et corrections progressives.

## Principes de développement

- Ne pas confondre « instructions CPU exécutées » et « jeu correctement émulé » : le timing et les composants vidéo/audio comptent.
- Écrire des tests ciblés pour chaque fonctionnalité avant d'élargir la compatibilité.
- Ne pas annoncer une fonctionnalité comme terminée sans test reproductible.
- Garder le cœur indépendant de l'interface graphique et du système d'exploitation.
- Respecter les droits d'auteur : ne pas ajouter de ROM commerciale au dépôt. Utiliser des ROM homebrew ou des suites de tests dont la licence autorise l'usage.

## Avancement

| Domaine | État |
|---|---|
| Structure et documentation initiales | En cours |
| CPU | À faire |
| Bus mémoire | À faire |
| Cartouches / mapper 0 | À faire |
| PPU | À faire |
| APU | À faire |
| Contrôleurs | À faire |
| Synchronisation complète | À faire |
| Tests de compatibilité | À faire |

Les pourcentages de complétion devront être basés sur des critères vérifiables, pas sur une estimation optimiste.
