# Implémentations de référence et stratégie de réutilisation

## Décision

PeaceEmu-NES doit **réutiliser des idées et des composants éprouvés** plutôt que réécrire chaque comportement matériel sans référence. La réutilisation doit rester ciblée : adapter les algorithmes au bus, aux types et à l'API de PeaceEmu, conserver des tests locaux et éviter de dépendre de l'interface graphique d'un autre émulateur.

Aucun fichier source tiers n'est importé dans le cœur par cette décision. Toute future reprise de code doit être identifiée dans ce document, accompagnée de sa provenance, de sa licence et des changements apportés.

## Référence principale étudiée : TetaNES

- Projet : [lukexor/tetanes](https://github.com/lukexor/tetanes)
- Composant matériel : [tetanes-core](https://github.com/lukexor/tetanes/tree/main/tetanes-core)
- Licence annoncée : MIT ou Apache-2.0 ; voir les fichiers [LICENSE-MIT](https://github.com/lukexor/tetanes/blob/main/LICENSE-MIT) et [LICENSE-APACHE](https://github.com/lukexor/tetanes/blob/main/LICENSE-APACHE).
- Zones pertinentes à comparer : CPU 6502 et modes d'adressage, séquencement PPU, registres de scroll, sprites, compteurs et canaux APU, lecture DMC et synchronisation bus/horloges.
- Limite d'intégration actuelle : la branche consultée exige Rust 1.88.0 pour `tetanes-core`, alors que PeaceEmu-NES cible Rust 1.78. Une dépendance directe à la version actuelle n'est donc pas acceptable sans changement explicite de la politique MSRV. Le cœur dépend aussi d'un modèle Bus/Mapper plus large que le nôtre ; l'ajouter comme dépendance ne remplacerait pas simplement `NesMachine`.

## Autres références examinées

- [RustyNES](https://github.com/doublegate/RustyNES) : émulateur Rust très complet, mais sous GPL-3.0. **Aucun code GPL n'est repris** dans ce dépôt ; son utilisation comme source de code nécessiterait une décision de licence explicite.
- [NESdev Wiki](https://www.nesdev.org/wiki/Nesdev_Wiki) : référence matérielle pour vérifier les comportements, sans importer de code source.

## Méthode de portage « recette maison »

1. Choisir un comportement matériel précis et documenter le problème observé.
2. Comparer au moins une implémentation permissive et les références matérielles.
3. Porter uniquement la logique utile, en l'adaptant à nos structures `Ppu`, `CpuBus`, `Nrom` et `NesMachine`.
4. Ajouter des tests de régression pour les cas normaux, les limites et les cas de synchronisation.
5. Ajouter ici la provenance et la licence si du code est effectivement repris ; conserver les notices exigées par la licence.
6. Valider formatage, tests et Clippy via CI avant d'annoncer le résultat.

## Première adaptation réalisée : LAX/SAX

- Référence consultée : [table des opcodes TetaNES](https://github.com/lukexor/tetanes/blob/main/tetanes-core/src/cpu/instr.rs), projet sous Apache-2.0 (le dépôt propose aussi le texte MIT).
- Adaptation PeaceEmu : prise en charge des formes stables LAX (A/X chargés ensemble) et SAX (écriture de A & X) dans `NES/src/cpu.rs`, avec calcul local des adresses, flags et cycles.
- Les lignes de code n'ont pas été copiées telles quelles : la logique a été réécrite pour notre interpréteur existant et couverte par des tests de régression locaux, y compris le franchissement de page pour LAX absolute,Y.
- Aucun code source tiers ni ROM de test tierce n'est embarqué par cette tranche. La table d'opcodes sert de référence de comportement ; les tests sont écrits pour PeaceEmu-NES.

## Deuxième adaptation réalisée : opcodes immédiats non officiels

- Référence de comportement : tables d'opcodes 2A03/6502 de TetaNES et documentation matérielle NESdev.
- Adaptation PeaceEmu : ANC ($0B/$2B), ALR ($4B), ARR ($6B) et AXS ($CB) sont désormais interprétés dans le CPU existant. Les opérations utilisent les helpers locaux de flags et les cycles de notre interpréteur ; aucune dépendance d'émulateur n'est ajoutée.
- Une régression dédiée vérifie les résultats de registres, les flags Carry/Zero/Negative/Overflow et les deux cycles de chaque instruction immédiate.
- Le comportement de ces opcodes non officiels varie parfois selon la famille de puces et les cas limites ; ils sont documentés comme couverture pratique 2A03, pas comme preuve de cycle-exactitude.

## Première tranche de validation

La PPU possède désormais des tests explicites pour les quatre alias de palette universelle (`$3F10/$3F14/$3F18/$3F1C`), les mappings horizontal/vertical/four-screen des nametables et le miroir de `$3000-$3EFF`. Ces tests sont des garde-fous avant l'évolution du rendu snapshot vers un pipeline de pixels et de fetchs par dot.

## Règle de compatibilité

Ne pas intégrer de code ou de dépendance uniquement parce qu'un émulateur est complet. Vérifier d'abord la licence, la version minimale de Rust, la structure des dépendances, l'API publique, les tests disponibles et l'impact sur la frontière `EmulationCore`. Le cœur PeaceEmu doit rester indépendant du frontend.

## Troisième adaptation réalisée : RMW composites non officiels

- Référence de comportement : tables d'opcodes 2A03/NESdev et comparaison avec l'organisation des instructions d'émulateurs Rust permissifs comme TetaNES.
- Adaptation locale : ajout des 42 formes stables des familles SLO, RLA, SRE, RRA, DCP et ISC/ISB, avec les sept modes d'adressage usuels et leurs cycles nominaux.
- Les opérations réutilisent les helpers locaux `shift`, `adc`, `compare`, `indirect_x` et `indirect_y`, sans nouvelle dépendance et sans copier un cœur externe.
- Une régression vérifie les effets mémoire, registres et flags pour les six familles. Les écritures factices et les effets de bus à chaque cycle restent hors périmètre : cette tranche n'est pas cycle-exacte.
