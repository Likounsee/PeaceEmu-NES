# PeaceEmu-NES — module NES

> Cœur d'émulation Nintendo Entertainment System (NES) pour l'application PeaceEmu existante.

## Objectif
## Réutilisation du code existant

Pour éviter de tout réimplémenter à la main, les références open source, leurs licences et les contraintes d'intégration sont suivies dans [la stratégie de réutilisation](docs/REFERENCE_IMPLEMENTATIONS.md). L'objectif est de porter des composants ciblés, de les adapter à l'architecture PeaceEmu et de vérifier chaque comportement par des tests — pas de copier un émulateur complet à l'aveugle.


Développer un cœur NES réel, indépendant de l'interface graphique, intégrable à l'application principale [PeaceEmu](https://github.com/Likounsee/PeaceEmu). L'interface, la bibliothèque de jeux, les fenêtres et l'orchestration des sessions existent déjà dans le dépôt principal : ce dépôt ne doit pas recréer ces fonctionnalités.

**État réel :** le crate Rust est créé. Le bus RAM, le parseur iNES et les mappers NROM, MMC1, UxROM, CNROM, MMC3, AxROM et GxROM, un sous-ensemble élargi du CPU, un modèle de registres APU, compteurs de longueur, séquenceur de frame approximatif et un mélangeur audio pulse/triangle/bruit/DMC à mixage non linéaire approximatif avec enveloppes, sweep et compteur linéaire triangle approximatifs, un premier canal DMC avec lecture d'échantillons par le bus et IRQ, ainsi que le rendu snapshot de la PPU disposent de code et de tests. Le comptage des frontières de scanline est désormais retourné par la PPU afin que les longues pauses DMA ne perdent pas les clocks IRQ MMC3 traversées. Le séquenceur APU fait avancer ses horloges frame à chaque cycle CPU pour garder un état déterministe quelle que soit la taille des lots passés à `tick_cpu_cycles`. Le module n'est pas encore déclaré jouable et n'est pas encore branché au contrat hôte.

## Langage et intégration

- **Langage :** Rust, en cohérence avec PeaceEmu.
- **Édition cible :** Rust 2021 ; respecter la version minimale et les conventions du dépôt hôte lors de l'intégration.
- **Responsabilité de ce dépôt :** matériel émulé et tests du cœur NES.
- **Responsabilité du dépôt PeaceEmu :** interface, bibliothèque, fenêtres, cycle de vie des sessions et services communs.
- **Contrat d'intégration :** adaptateur à écrire pour implémenter EmulationCore, fournir un identifiant système stable, déclarer les capacités réellement prises en charge et transférer images/audio via les types communs.
- **Point à résoudre avant le branchement compilable :** le crate emulation-core se trouve actuellement dans le workspace du dépôt hôte et n'est pas publié comme dépendance. Il faut choisir une stratégie de partage/versionnement du contrat au lieu de dupliquer ses types.

## Réutilisation de références existantes

Le développement s'appuie sur des émulateurs NES existants et des références matérielles plutôt que de réécrire chaque détail isolément. Le CPU intègre maintenant des variantes stables non officielles LAX/SAX et les opérations immédiates ANC, ALR, ARR et AXS, adaptées à nos registres, à notre bus et à nos tests. Les projets étudiés, leurs licences, leurs limites de compatibilité et la méthode de portage sont documentés dans [REFERENCE_IMPLEMENTATIONS.md](docs/REFERENCE_IMPLEMENTATIONS.md). Cette démarche privilégie des ports ciblés et testés : aucune dépendance d'émulateur complète n'est ajoutée aveuglément, et les composants PeaceEmu gardent leurs propres interfaces.

## Implémentation actuelle

- **CPU :** sous-ensemble élargi des instructions officielles : chargements/stores, ADC/SBC, AND/ORA/EOR, comparaisons, BIT, shifts/rotates, incréments/décréments, branchements, JMP indirect avec comportement de page 6502, pile, JSR/RTS, RTI et BRK. Plusieurs opcodes non officiels stables sont aussi reconnus : LAX/SAX/LAS, ANC, ALR, ARR, AXS, les opérations composites SLO/RLA/SRE/RRA/DCP/ISC et des NOP avec lectures mémoire, opérandes et pénalités de franchissement de page ; les opcodes encore inconnus renvoient une erreur explicite. Ce n'est pas encore un CPU complet ni cycle-exact.
- **Bus :** 2 Kio de RAM interne avec miroirs et latch open-bus minimal ; la zone cartouche `$6000–$7FFF` route vers les 8 Kio de PRG-RAM NROM, accessible en lecture/écriture ; registres PPU ($2000–$3FFF), DMA OAM ($4014), APU ($4000–$4017 hors adresses réservées), compteurs de longueur, séquenceur de frame approximatif, premiers registres DMC/lecture d'échantillon et deux manettes série ($4016/$4017) routés. Les manettes exposent huit boutons par port, latch sur strobe et décalage sériel avec ligne haute après le huitième bit.
- **PPU :** première couche matérielle : registres CPU, adresse VRAM, buffer de lecture, mémoire nametable/palette, OAM, mirroring de cartouche et drapeau VBlank/compteur de scanline. Le DMA OAM copie 256 octets d'une page CPU vers l'OAM et ajoute 513/514 cycles de blocage selon l'alignement. Un rendu par instantané est disponible via `NesMachine::render_background()` ou `NesMachine::render_frame()` et `NesMachine::framebuffer()` : le framebuffer de 256×240 pixels contient des indices de palette NES 6 bits. Le rendu lit les tuiles, plans de bits, attributs de palette, mirroring et défilement de base ; `render_frame()` compose aussi les sprites avec flips horizontal/vertical, tailles 8×8/8×16, priorité approximative, hit sprite 0 et overflow de sprites par ligne. Le rendu snapshot recalcule les indicateurs hit/overflow à chaque rendu afin d’éviter les résultats périmés entre deux snapshots. Ce n'est pas encore une pipeline dot-par-dot. Le NMI VBlank est routé vers l'entrée d'interruption CPU via une ligne NMI à détection de front montant (pas de retrigger lors d'écritures répétées de PPUCTRL tant que la ligne reste haute) ; l'IRQ frame APU est aussi exposée comme ligne niveau et prise en compte aux frontières d'instructions quand I=0 ; le CPU donne la priorité au NMI et conserve l'IRQ APU en attente ; le compteur NTSC modélise aussi le saut de dot du pré-rendu lors des images impaires avec rendu actif.
- **Cartouche :** parsing iNES legacy et dispatch statique pour NROM (mapper 0), MMC1 (mapper 1), UxROM (mapper 2), CNROM (mapper 3) et MMC3 (mapper 4), AxROM (mapper 7) et GxROM (mapper 66). MMC3 fournit les fenêtres PRG de 8 Kio, les banques CHR de 1/2 Kio, la protection PRG-RAM, le mirroring commutable et les registres d’IRQ scanline (horloge encore approximée par ligne). GxROM commute indépendamment les fenêtres PRG de 32 Kio et CHR de 8 Kio, avec limites de banques et mirroring iNES fixe. MMC1 prend en charge le registre série cinq bits, les modes PRG 16/32 Kio, les banques CHR 4 Kio/8 Kio, le désactivation de PRG-RAM par registre, le mirroring contrôlé (one-screen/vertical/horizontal) et CHR-RAM quand aucune CHR-ROM n'est déclarée. Les mappers ont des tests de banque, limites, mirroring et écritures. Les trainers iNES sont copiés en PRG-RAM à `$7000–$71FF`. Les variantes à bus conflicts, NES 2.0 et les autres mappers ne sont pas encore pris en charge.
- **Machine :** `NesMachine` charge les cartouches iNES supportées, initialise le CPU depuis le vecteur de reset et expose le pas d'instruction ainsi que les lectures/écritures CPU. Le chargement et la commutation de banques MMC1 sont testés de bout en bout via le bus CPU.
- **Tests :** la suite couvre aussi les opcodes non officiels LAX/SAX, ANC, ALR, ARR, AXS et les NOP (modes immédiat, zéro-page, absolu et absolu,X avec franchissement de page), la PRG-RAM cartouche, le chargement du trainer à `$7000`, le rejet des tailles CHR incompatibles avec NROM, et l'accès à la PRG-RAM via le bus CPU, en plus des régressions CPU/APU/PPU/manettes précédentes.
- **CI :** workflow GitHub Actions exécute formatage, tests et Clippy. Le statut doit être confirmé sur le commit final du lot.

## Ordre de développement

1. Étendre le rendu PPU vers une pipeline par dot. Les incréments coarse X/fine Y, le transfert horizontal au dot 257 et le transfert vertical au pré-rendu sont maintenant déclenchés aux points de timing prévus quand le rendu est activé ; les fetchs de pixels restent snapshot-based, et les collisions/overflow ne sont pas encore évalués en temps réel.
2. Compléter le CPU : CMP dispose maintenant des modes indirect X, zéro-page et indirect Y avec pénalité de franchissement de page ; RTI restaure le statut et le PC depuis la pile. Les modes AND absolu,Y et indirect,Y appliquent aussi la pénalité de franchissement de page. Poursuivre les instructions officielles restantes et leurs cycles.
3. Raffiner l’APU : le DMC initial inclut maintenant lecture mémoire, buffer, unité delta, bouclage et IRQ, et son octet est servi à l’échelle du cycle APU. Les tests couvrent aussi l'arrêt du DMC, l'effacement des octets restants et l'acquittement IRQ par $4015 ; restent l'arbitrage/stall DMA cycle-exact, le filtrage, le resampling à fréquence hôte et un timing plus fidèle. Raffiner le timing DMA/NMI.
4. Intégrer le contrat hôte PeaceEmu sans dépendance graphique dans le cœur.
5. Ajouter sauvegardes et tests de compatibilité reproductibles.
6. Étendre progressivement les mappers à partir de références permissives et des spécifications NESdev ; mapper 71 Camerica est désormais ajouté dans une adaptation locale testée.

## Principes

- Ne pas recréer une interface graphique ou une bibliothèque de jeux dans ce dépôt.
- Garder le matériel NES indépendant de l'interface et du système d'exploitation.
- Ne pas confondre instructions CPU exécutées et jeu correctement émulé : le timing et les composants vidéo/audio comptent.
- Ajouter des tests ciblés avec chaque fonctionnalité ; ne pas supprimer de tests de régression pour faire passer la CI.
- Ne pas déclarer une capacité prise en charge avant son implémentation et ses tests.
- Ne pas ajouter de ROM commerciale au dépôt. Utiliser des homebrews ou des suites de tests dont la licence autorise l'usage.

## Avancement

**Avancement global estimé : 73 %.** La machine minimale charge une cartouche NROM, exécute un sous-ensemble du CPU, et la PPU expose les registres, la mémoire graphique, le timing VBlank/NMI, des incréments/copies de scroll synchronisés à certains dots, des fetchs de tuiles en fin de ligne (dots 328 et 336) et un rendu par instantané. Les indicateurs sprite hit/overflow sont réinitialisés et recalculés lors de chaque rendu. `NesMachine::step_frame()` avance désormais jusqu'à la prochaine frontière de frame, rend une image et renvoie le nombre nominal de cycles CPU consommés ; le passage initial du pré-rendu est ignoré au démarrage, et un budget de sécurité dépassé renvoie une erreur explicite. Une suite de tests couvre les composants de base, le transfert DMA des 256 octets avec ses deux parités de blocage (513/514 cycles), la progression PPU à trois dots par cycle CPU, l'entrée NMI, le décodage des pixels, la priorité des sprites, le protocole série des manettes, les mirroring nametables/palettes et le saut de dot des images impaires, les incréments de scroll aux dots 256/257/328/336 et le recalcul de l'overflow sprite. L’APU expose un échantillon mono normalisé et sa ligne IRQ frame est désormais prise en compte par le CPU lorsque les interruptions sont autorisées ; le CPU empile le PC et le statut, puis charge le vecteur IRQ. L’APU modélise approximativement les enveloppes pulse, le sweep, le compteur linéaire du triangle et un premier canal DMC ; les modes 4/5 étapes et l’effacement de l’IRQ frame par inhibition ont des tests dédiés : registres $4010–$4013, adresse et longueur d'échantillon, buffer, sortie delta 7 bits, démarrage silencieux, bouclage et IRQ DMC. Le bus alimente le buffer depuis l'espace d'adressage CPU entre les cycles APU et les tests couvrent le chargement du buffer, le démarrage silencieux et le wrap d'adresse ; les lectures DMC n'appliquent pas encore de cycles de blocage/arbitrage précis ; le mixage et le timing restent approximatifs, avec resampling à fréquence hôte à faire. Des tests supplémentaires couvrent le masquage des huit pixels de gauche du fond, l'acquittement de l'IRQ frame en activant l'inhibition, le passage entre séquenceurs quatre/cinq étapes et le port PPUDATA (buffer et incrément de 1/32), le DAC DMC, le bouclage, le retour d'adresse et les transferts mémoire par le bus. La lecture de `$2002` teste aussi le reset du latch partagé et le nettoyage du VBlank au pré-rendu ; un test vérifie les fronts NMI et la priorité du NMI sur une IRQ APU en attente. Le timing reste approximatif et la compatibilité de jeux n’est pas encore établie. Ce pourcentage mesure l'avancement du développement, pas la compatibilité des jeux : aucune ROM commerciale ou homebrew complète n'est encore déclarée compatible.

Le dernier lot ajoute Camerica (mapper 71) : banque PRG commutable en $8000-$BFFF, dernière banque fixe, mirroring one-screen sur écriture $9000 et CHR-RAM, avec tests dédiés. Le cœur passe de 7 à 8 familles de mappers, sans revendiquer la prise en charge de toutes les variantes de cartes. La correction PPU/MMC3 et l'opcode LAS restent couverts par les tests existants.

| Domaine | État |
|---|---|
| Structure Rust et build initial | En place |
| CPU Ricoh 2A03 | Partiel — sous-ensemble élargi, pile/flags/transferts/branches ; entrée NMI et IRQ masquable APU implémentées, tests de drapeaux PHP/PLP renforcés |
| Bus RAM, DMA OAM et manettes | RAM miroir, registres PPU, transfert DMA/blocage CPU, deux ports série manette |
| Cartouches iNES legacy | NROM, MMC1, UxROM, CNROM, MMC3 (mapper 4), AxROM, GxROM (mapper 66) et Camerica (mapper 71, sous-ensemble iNES/CHR-RAM) ; NES 2.0 et autres mappers à faire |
| Machine et reset vector | Première version — charge NROM, exécute le CPU, synchronise les dots PPU à 3:1 et gère les deux parités de stall DMA OAM |
| PPU | Registres/mémoire/VBlank-NMI ; incréments/copies de scroll aux points clés, y compris fetchs 328/336 ; hit/overflow sprite recalculés ; rendu snapshot, pipeline pixel/fetch par dot à faire |
| APU | Partiel — registres, compteurs, enveloppes pulse, sweep, compteur linéaire triangle et premier DMC (lecture mémoire, buffer, démarrage silencieux, sortie delta, bouclage/IRQ, arrêt/acquittement testés) ; arbitrage DMA, filtrage, resampling et timing exact à faire |
| Contrôleurs | Première version — ports série $4016/$4017, huit boutons et latch |
| Timing complet | À faire |
| Adaptateur EmulationCore | À faire — partage du contrat à définir |
| Tests de compatibilité | À faire |
