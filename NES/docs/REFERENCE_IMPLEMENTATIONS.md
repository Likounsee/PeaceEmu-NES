# Implémentations de référence et stratégie de réutilisation

## Décision

PeaceEmu-NES doit **réutiliser des idées et des composants éprouvés** plutôt que réécrire chaque comportement matériel sans référence. La réutilisation doit rester ciblée : adapter les algorithmes au bus, aux types et à l'API de PeaceEmu, conserver des tests locaux et éviter de dépendre de l'interface graphique d'un autre émulateur.

Aucun fichier source tiers n'est importé dans le cœur par cette décision. Toute future reprise de code doit être identifiée dans ce document, accompagnée de sa provenance, de sa licence et des changements apportés.

## Référence principale étudiée : TetaNES

- Projet : [lukexor/tetanes](https://github.com/lukexor/tetanes)
- Composant matériel : [tetanes-core](https://github.com/lukexor/tetanes/tree/main/tetanes-core)
- Licence annoncée : MIT ou Apache-2.0 ; voir les fichiers [LICENSE-MIT](https://github.com/lukexor/tetanes/blob/main/LICENSE-MIT) et [LICENSE-APACHE](https://github.com/lukexor/tetanes/blob/main/LICENSE-APACHE).
- Zones pertinentes à comparer : CPU 6502 et modes d'adressage, séquencement PPU, registres de scroll, sprites, compteurs et canaux APU, lecture DMC et synchronisation bus/horloges.
- Limite d'intégration actuelle : la branche consultée exige Rust 1.88.0 pour `tetanes-core`, PeaceEmu-NES suit désormais le canal `stable` via `NES/rust-toolchain.toml` et déclare Rust 1.99 comme version minimale dans `NES/Cargo.toml`. Une dépendance directe reste à évaluer selon son modèle Bus/Mapper et son API, pas seulement selon sa version de Rust. Le cœur dépend aussi d'un modèle Bus/Mapper plus large que le nôtre ; l'ajouter comme dépendance ne remplacerait pas simplement `NesMachine`.

## Autres références examinées

- [RustyNES](https://github.com/doublegate/RustyNES) : émulateur Rust très complet, mais sous GPL-3.0. **Aucun code GPL n'est repris** dans ce dépôt ; son utilisation comme source de code nécessiterait une décision de licence explicite.
- [NESdev Wiki](https://www.nesdev.org/wiki/Nesdev_Wiki) : référence matérielle pour vérifier les comportements, sans importer de code source.

## Méthode de portage « recette maison »

1. Choisir un comportement matériel précis et documenter le problème observé.
2. Comparer au moins une implémentation permissive et les références matérielles.
3. Porter uniquement la logique utile, en l'adaptant à nos structures `Ppu`, `CpuBus`, `Cartridge` et `NesMachine`.
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



## Quatrième adaptation réalisée : abstraction mapper et UxROM (mapper 2)

- Référence de conception : implémentation UxROM de TetaNES et documentation matérielle NESdev ; les références ont servi à confirmer la disposition « banque commutable en $8000-$BFFF, dernière banque fixe en $C000-$FFFF ».
- Adaptation locale : ajout de `Cartridge` comme enum de dispatch statique et d'une implémentation `Uxrom`. Le chargement iNES choisit désormais NROM (mapper 0) ou UxROM (mapper 2) ; le bus CPU route les écritures mapper, et la PPU utilise l'interface commune du cartridge.
- Tests de régression : sélection de banques, banque finale fixe, limites $8000/$BFFF/$C000/$FFFF, PRG-RAM, CHR-RAM et chargement UxROM via `NesMachine` avec le vecteur de reset dans la banque fixe.
- Limites explicites : NES 2.0 et CHR-ROM multi-banque restent non pris en charge ; les variantes UxROM avec bus conflicts ne sont pas encore modélisées. Cette tranche est une première prise en charge mapper 2, pas une compatibilité exhaustive de toutes les cartouches UxROM.
- Aucun code source TetaNES n'a été copié dans cette tranche ; la structure et les comportements matériels ont été adaptés au code et aux tests de PeaceEmu-NES.


## Cinquième adaptation réalisée : CNROM (mapper 3)

- Référence de conception : modèle de commutation CHR du mapper 3 décrit par NESdev, comparé à l'organisation des mappers dans TetaNES.
- Adaptation locale : ajout de `Cnrom` dans le dispatch statique `Cartridge`. Le PRG reste mappé comme NROM (16 ou 32 Kio) et les écritures CPU dans `$8000-$FFFF` sélectionnent l'une des banques CHR-ROM de 8 Kio.
- Tests : commutation des banques CHR aux bornes `$0000`/`$1FFF`, stabilité du mapping PRG, protection de la CHR-ROM en écriture et rejet des images sans CHR-ROM ou avec plus de quatre banques CHR.
- Limites : CNROM à bus conflicts non simulé ; seuls les iNES classiques avec 1 ou 2 banques PRG et 1 à 4 banques CHR sont acceptés. NES 2.0 reste hors périmètre.
- Comme pour UxROM, aucune source de TetaNES n'a été copiée : les comportements sont adaptés à l'interface locale et testés dans PeaceEmu-NES.

## Sixième adaptation réalisée : AxROM (mapper 7)

- Références de comportement : [documentation matérielle NESdev sur AxROM/mapper 7](https://www.nesdev.org/wiki/AxROM), confrontée aux conventions de banques et de mirroring de TetaNES.
- Adaptation locale : fenêtres PRG de 32 Kio sélectionnées par les bits 0–3 des écritures CPU, CHR-RAM de 8 Kio, et mirroring one-screen basculé par le bit 4. La PPU connaît maintenant les deux pages one-screen en plus des modes horizontal, vertical et four-screen.
- Tests : sélection de banques PRG aux deux extrémités de la fenêtre, bascule de page nametable, écriture/lecture CHR-RAM et rejet des tailles PRG impaires/hors plage ainsi que de CHR-ROM.
- Limites : uniquement iNES classique, 2 à 16 unités PRG de 16 Kio (nombre pair), CHR-RAM et pas de modélisation de variantes de carte ou de bus conflicts. Aucune source TetaNES n'est copiée : les règles matérielles sont réimplémentées dans les types locaux.

## Politique Rust

- `NES/rust-toolchain.toml` sélectionne `stable` et installe `rustfmt`/`clippy`, afin que les outils locaux suivent la dernière version stable disponible avec rustup.
- `NES/Cargo.toml` fixe `rust-version = "1.99"`, la version minimale compatible visée par le projet depuis la mise à niveau vers Rust 1.99.x.
- La CI utilise également `dtolnay/rust-toolchain@stable` : elle valide donc le cœur avec la dernière stable disponible au moment de l'exécution, pas avec Nightly.

## Septième adaptation réalisée : MMC1 (mapper 1)

- Références de comportement : documentation matérielle NESdev sur MMC1 et organisation des registres/banques observée dans TetaNES (licence permissive MIT/Apache-2.0).
- Adaptation locale dans `NES/src/cartridge.rs` : registre série 5 bits et bit de reset, modes PRG 16/32 Kio, banques CHR 4/8 Kio, désactivation de PRG-RAM, et mirroring one-screen/vertical/horizontal. Le dispatch statique `Cartridge` et le bus existant restent inchangés dans leur contrat public.
- Tests ajoutés : sélection PRG dans les modes commutables/fixes, sélection des deux fenêtres CHR 4 Kio, écriture CHR-RAM, mirroring, protection PRG-RAM et reset du registre série. Un test au niveau `NesMachine` vérifie également le chargement iNES, le vecteur de reset et les écritures sérielles à travers le bus CPU.
- Limites : images iNES classiques uniquement ; cette version accepte 2, 4, 8 ou 16 banques PRG de 16 Kio et jusqu'à 16 banques CHR de 8 Kio (ou CHR-RAM). Les variantes MMC1 spécifiques à certaines cartes, la gestion temporelle des écritures consécutives, l'horloge cycle-exacte et NES 2.0 ne sont pas encore modélisées. Aucun code source TetaNES n'a été copié : les règles matérielles ont été adaptées aux types et méthodes PeaceEmu.


## Huitième adaptation réalisée : GxROM (mapper 66)

- Référence de conception : comportement public du mapper GxROM décrit par [NESdev — GxROM](https://www.nesdev.org/wiki/GxROM), utilisé pour vérifier le décodage du registre de banques.
- Adaptation PeaceEmu : nouveau type Rust `Gxrom` intégré à l'enum `Cartridge`. Les bits 4–5 sélectionnent une fenêtre PRG de 32 Kio et les bits 0–1 une banque CHR de 8 Kio ; le nombre réel de banques borne les valeurs sélectionnées. La RAM PRG, le mirroring iNES et les trainers sont traités par le chargeur commun au mapper.
- Tests ajoutés : commutation PRG/CHR indépendante, limites de banques sur les petites images, mirroring horizontal/vertical, refus d'écriture CHR-ROM, tailles invalides et image tronquée.
- Limites : prise en charge iNES legacy seulement, CHR-ROM obligatoire, sans modélisation des variantes matérielles ni NES 2.0. Le comportement est adapté aux interfaces locales PeaceEmu ; aucun code source tiers n'a été copié.


## Neuvième adaptation réalisée : MMC3 / TxROM (mapper 4)

- Référence comportementale : [NESdev — MMC3](https://www.nesdev.org/wiki/MMC3) et [NESdev — mapper 004](https://www.nesdev.org/wiki/INES_Mapper_004), pour l'organisation des fenêtres PRG de 8 Kio, des registres CHR, de la protection PRG-RAM, du mirroring et du compteur IRQ.
- Adaptation PeaceEmu : nouveau type Rust `Mmc3` dans le dispatch statique `Cartridge`. Les registres R0–R7 commandent les banques PRG/CHR ; les bits de mode inversent les fenêtres attendues ; les écritures $A001 protègent/désactivent la RAM ; $A000 règle le mirroring sauf pour le four-screen ; $C000/$C001 et $E000/$E001 contrôlent le compteur et l'acquittement IRQ.
- Tests ajoutés : fenêtres PRG commutables/fixes, mode PRG inversé, banques CHR 1/2 Kio, CHR-RAM, protection de RAM, mirroring four-screen, chargement/activation/effacement IRQ.
- Limites : l'horloge IRQ est pour l'instant approximée à un changement de scanline de la PPU ; le filtre matériel sur A12, le timing précis des fetchs PPU, les variantes MMC3A/B/C et NES 2.0 restent à implémenter. Le comportement est adapté aux interfaces PeaceEmu ; aucun code source tiers n'a été copié.


## Correction de synchronisation PPU/MMC3

- La PPU renvoie maintenant le nombre de frontières de scanline traversées par chaque lot de dots, y compris le passage de fin de frame et le saut de pré-rendu des frames impaires.
- `NesMachine::step` transmet chaque frontière au compteur IRQ MMC3. Avant cette correction, un seul appel CPU avançant plusieurs scanlines (notamment une pause DMA OAM) ne déclenchait qu'une seule clock IRQ.
- Tests locaux couvrent le comptage multi-scanline, le retour de frame et le saut de dot NTSC impair.
- Limite restante explicitement conservée : l'IRQ MMC3 est encore approximée par frontière de scanline, pas par fronts PPU A12 filtrés. Cette tranche corrige la perte de frontières sans prétendre rendre le pipeline PPU cycle-exact.


## Politique de réutilisation et opcodes CPU supplémentaires

- Les émulateurs Rust existants servent à identifier les comportements et les cas limites. RustyNES (`GPL-3.0-or-later`) et nesium (`GPL-3.0`) sont des références utiles pour leur architecture et leur stratégie de tests, mais leurs fichiers source ne sont pas copiés dans ce cœur.
- La compatibilité de licence est vérifiée avant tout port de code. Les spécifications NESdev et les ROM de diagnostic dont la licence autorise l'usage restent les sources de vérité privilégiées ; une sortie d'émulateur peut servir d'oracle comparatif sans réutiliser son implémentation.
- Le CPU local reconnaît désormais aussi LAS (`$BB`) : il charge A, X et le pointeur de pile depuis la valeur mémoire ET le pointeur de pile, avec pénalité de franchissement de page en mode absolu,Y. Le test couvre les deux cas, avec et sans changement de page.
- Cette amélioration complète le groupe déjà présent d'opcodes non officiels stables (LAX/SAX et opérations composites), sans prétendre couvrir les opcodes instables ni rendre l'exécution cycle-exacte.


## Dixième adaptation réalisée : Camerica (mapper 71)

- Référence de comportement : documentation NESdev du mapper 71/Camerica et comparaison de la structure PRG avec le mapper UxROM déjà présent. Les références externes servent à vérifier les fenêtres mémoire et les cas limites ; aucun code source d'émulateur n'a été importé.
- Adaptation locale : `Camerica` fournit une banque PRG de 16 Kio commutable dans $8000-$BFFF, une dernière banque fixe dans $C000-$FFFF, 8 Kio de CHR-RAM, PRG-RAM et la sélection one-screen sur les écritures de la zone $9000. Le chargement reste limité aux images iNES legacy, 2 à 16 banques PRG et CHR-RAM.
- Tests : banque basse/fixe, bornes CPU, mirroring vertical initial et one-screen supérieur, lecture/écriture CHR-RAM, et rejet des tailles PRG/CHR non prises en charge.
- Limites : les variantes de cartes Camerica et leurs décodages précis ne sont pas toutes modélisées ; le test valide notre sous-ensemble documenté, pas une compatibilité commerciale exhaustive.


## Onzième adaptation réalisée : Color Dreams (mapper 11)

- Référence de comportement : spécification publique NESdev du mapper Color Dreams, comparée au modèle de commutation PRG/CHR déjà présent pour GxROM (mapper 66).
- Adaptation locale : un registre d'écriture sélectionne une fenêtre PRG de 32 Kio via les bits 0–3 et une banque CHR-ROM de 8 Kio via les bits 4–7. La RAM PRG, le mirroring iNES, les trainers et les bornes de ROM suivent les conventions locales de `Cartridge`.
- Tests : commutation PRG et CHR indépendante, limites des deux fenêtres, protection en écriture de CHR-ROM, absence de CHR-ROM et tailles PRG non prises en charge.
- Limites : iNES legacy uniquement ; NES 2.0 et les variantes de cartes ne sont pas implémentés. Aucune source d'émulateur externe n'a été copiée.


## Douzième adaptation réalisée : BNROM (mapper 34)

- Référence de comportement : documentation publique NESdev du mapper 34, comparée au commutateur PRG 32 Kio de GxROM et aux modèles CHR-RAM déjà présents.
- Adaptation locale : variante BNROM uniquement ; une écriture dans $8000-$FFFF sélectionne la fenêtre PRG de 32 Kio, la CHR-RAM de 8 Kio est accessible en lecture/écriture, et le mirroring vient de l'en-tête iNES. Les trainers sont copiés en PRG-RAM selon la convention locale.
- Tests : commutation de toute la fenêtre PRG, mirroring, accès aux deux extrémités CHR-RAM et rejet des tailles PRG/CHR non prises en charge.
- Limite importante : le mapper 34 peut aussi désigner des cartes NINA-001 selon l'image et le sous-mapper. Cette implémentation ne prétend prendre en charge que BNROM ; NINA-001 n'est pas traité.
- Aucun code source d'émulateur externe n'a été copié ; l'implémentation est écrite selon les conventions du crate PeaceEmu-NES.


## Treizième adaptation réalisée : CPROM (mapper 13)

- Référence de comportement : documentation matérielle publique NESdev sur les cartes CPROM/mapper 13, comparée à la gestion locale des banques CHR de CNROM et Color Dreams.
- Adaptation « recette maison » : 32 Kio de PRG-ROM fixes, 16 Kio de CHR-RAM séparés en une zone basse fixe de 4 Kio et quatre banques commutables de 4 Kio dans la zone haute. Une écriture CPU dans $8000-$FFFF sélectionne la banque haute ; PRG-RAM, trainer et mirroring iNES suivent les conventions du crate.
- Tests : PRG fixe sur les deux fenêtres, isolation et persistance des quatre banques CHR hautes, zone basse fixe, refus de CHR-ROM et de tailles PRG autres que 32 Kio.
- Limites : uniquement l'organisation CPROM legacy iNES décrite ici ; NES 2.0 et variantes de cartes restent non pris en charge. Aucun code source d'un émulateur tiers n'a été copié ; les règles matérielles ont été réimplémentées dans l'enum de dispatch PeaceEmu.


## Quatorzième adaptation réalisée : stores instables du 2A03

- Références de comportement : tableaux d'opcodes 6502/2A03 et documentation NESdev sur les instructions non officielles ; comparaison conceptuelle avec les interpréteurs existants, sans importer leur code source.
- Adaptation PeaceEmu : ajout de AHX indirect-indexé (`$93`) et absolu,Y (`$9F`), TAS (`$9B`), SHY (`$9C`) et SHX (`$9E`). Les valeurs sont masquées par l'octet haut de l'adresse de base augmenté de un ; lorsqu'un index franchit une page, le modèle fait aussi refléter la corruption d'octet haut par la valeur écrite.
- Vérifications : tests dédiés aux deux formes AHX, au changement de SP par TAS, aux masques de données et au cas de franchissement de page. CI verte sur le commit `76521b9`.
- Limite importante : les instructions dites instables dépendent de détails électriques du processeur et peuvent avoir des comportements moins uniformes selon les cas. Ce modèle vise les comportements documentés courants ; il ne constitue pas une exécution bus-cycle-exacte du 2A03.
- Méthode : conserver le dispatch et les helpers de PeaceEmu, comparer les résultats aux tableaux matériels, puis verrouiller les cas connus avec des tests. Pas de dépendance d'émulateur ajoutée et pas de code tiers copié.


## Quinzième adaptation réalisée : MMC2 (mapper 9)

- Références de comportement : [NESdev — MMC2](https://www.nesdev.org/wiki/MMC2) et les règles de disposition PRG/CHR des cartouches iNES, comparées aux interfaces de commutation déjà implémentées dans PeaceEmu-NES.
- Adaptation locale : une fenêtre PRG de 8 Kio sélectionnable en $8000-$9FFF et trois fenêtres fixes en $A000-$FFFF ; quatre registres CHR de 4 Kio commandés par deux latches FD/FE ; mirroring horizontal/vertical commutable, tout en préservant le four-screen déclaré dans l'en-tête. Les transitions de latch sont suivies avec `Cell` dans la lecture CHR afin de conserver la signature de lecture utilisée par le rendu PPU existant.
- Tests : banques PRG sélectionnables/fixes, changement des deux latches par accès aux adresses de déclenchement, mirroring et protection four-screen, refus de PRG trop petit ou de CHR-ROM absente, et dispatch à travers `Cartridge`.
- Limites : mapper MMC2 uniquement sur iNES legacy avec CHR-ROM ; NES 2.0, variantes matérielles et timing PPU par dot restent hors périmètre. Les références existantes ont guidé le comportement ; aucun fichier source d'émulateur n'a été copié.


## Seizième adaptation réalisée : UNROM-512 (mapper 30)

- Référence matérielle principale : [NESdev — UNROM 512](https://www.nesdev.org/wiki/UNROM512), notamment le registre `[NCCPPPPP]`, les fenêtres PRG de 16 Kio, les quatre banques CHR-RAM de 8 Kio et l'absence de PRG-RAM sur la carte décrite. Le fonctionnement des fenêtres a aussi été comparé au mapper UxROM déjà présent dans PeaceEmu.
- Adaptation « recette maison » : intégration de `Unrom512` dans l'enum `Cartridge`, avec PRG sélectionnable par les bits 0–4, dernière banque fixe, CHR-RAM 32 Kio commutable par les bits 5–6 et bit 7 de mirroring one-screen lorsque le drapeau iNES indique une configuration commutable. Le chargement saute correctement le trainer avant la PRG-ROM, sans prétendre fournir de PRG-RAM physique.
- Tests : banques PRG commutables/fixes, persistance isolée des quatre banques CHR-RAM, mirroring one-screen, trainer et refus de PRG-ROM hors capacités retenues ou de CHR-ROM.
- Limites explicites : sous-ensemble iNES legacy (256/512 Kio PRG) ; sous-mappers NES 2.0, flash ROM, conflits de bus et variantes de configuration H/V/four-screen non entièrement modélisés. Le comportement est réimplémenté dans les structures PeaceEmu, sans copier le code d'un émulateur tiers.
