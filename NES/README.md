# PeaceEmu-NES — module NES

> Cœur d'émulation Nintendo Entertainment System (NES) destiné à être intégré dans l'application PeaceEmu existante.

## Architecture cible : PeaceEmu contrôle, fenêtre de jeu dédiée

PeaceEmu reste le centre de contrôle : il choisit la ROM, les touches, la mise à l'échelle et les options audio, puis lance le module avec ces réglages. Le module ouvre **une fenêtre séparée qui ne contient que l'image du jeu** (HUD original inclus s'il fait partie du jeu), sans barre d'outils ni HUD ajouté par l'émulateur. Le frontend Windows natif actuel sert de chemin de jeu immédiat ; l'API `NesSession` permet aussi l'intégration future au contrat partagé `EmulationCore`.

### Lancement direct sur Windows

Depuis le dossier `NES`, avec Rust installé :

```powershell
cargo run --release --bin peaceemu-nes-desktop -- "C:\chemin\vers\jeu.nes" --scale 3 --audio-rate 44100 --keys 90,88,16,13,38,40,37,39 --keys2 97,99,96,110,104,98,100,102
```

- `--scale 1..6` règle l'agrandissement entier de l'image (défaut : 3×).
- `--audio-rate 8000..192000` règle le taux PCM (défaut : 44 100 Hz).
- `--keys A,B,SELECT,START,UP,DOWN,LEFT,RIGHT` configure la manette 1 (défaut : `90,88,16,13,38,40,37,39`).
- `--keys2 A,B,SELECT,START,UP,DOWN,LEFT,RIGHT` configure la manette 2 (défaut : `97,99,96,110,104,98,100,102`, pavé numérique).
- Les entrées sont ignorées quand la fenêtre de jeu n'est pas au premier plan : le clavier de PeaceEmu ne déclenche donc pas les commandes du jeu en arrière-plan.
- Flèches : directions ; **Z** : A ; **X** : B ; **Maj droite** : Select ; **Entrée** : Start.
- Les sauvegardes RAM des cartouches avec batterie sont chargées/enregistrées dans un fichier `.sav` à côté de la ROM.
- Le son utilise la sortie PCM Windows ; si aucun périphérique audio n'est disponible, le jeu continue en silence.

PeaceEmu pourra transmettre ses réglages au lancement en construisant ces arguments ; l'UI de configuration reste dans PeaceEmu, jamais dans la fenêtre de jeu.

### API d'intégration du cœur

Dans `src/session.rs`, `NesSession::load(rom, config)` charge une ROM, `set_controller_state(...)` transmet les entrées, `advance_frame()` renvoie les pixels NES indexés (256×240) et les échantillons audio PCM, et les méthodes de RAM batterie laissent l'hôte gérer le stockage. L'adaptateur vers le contrat partagé `EmulationCore` reste à brancher.

## Objectif
## Réutilisation du code existant

Pour éviter de tout réimplémenter à la main, les références open source, leurs licences et les contraintes d'intégration sont suivies dans [la stratégie de réutilisation](docs/REFERENCE_IMPLEMENTATIONS.md). L'objectif est de porter des composants ciblés, de les adapter à l'architecture PeaceEmu et de vérifier chaque comportement par des tests — pas de copier un émulateur complet à l'aveugle.


Développer un cœur NES réel, indépendant de l'interface graphique, intégrable à l'application principale [PeaceEmu](https://github.com/Likounsee/PeaceEmu). L'interface, la bibliothèque de jeux, les fenêtres et l'orchestration des sessions existent déjà dans le dépôt principal : ce dépôt ne doit pas recréer ces fonctionnalités.

**État réel :** le crate Rust est créé. Le bus RAM, le parseur iNES et les mappers NROM, MMC1, UxROM, CNROM, MMC3, MMC2, AxROM, GxROM, CPROM et UNROM-512, un sous-ensemble élargi du CPU, un modèle de registres APU, compteurs de longueur, séquenceur de frame approximatif et un mélangeur audio pulse/triangle/bruit/DMC à mixage non linéaire approximatif avec enveloppes, sweep et compteur linéaire triangle approximatifs, un premier canal DMC avec lecture d'échantillons par le bus et IRQ, ainsi que le rendu par ligne de la PPU synchronisé sur la progression du faisceau disposent de code et de tests. Le comptage des frontières de scanline est désormais retourné par la PPU afin que les longues pauses DMA ne perdent pas les clocks IRQ MMC3 traversées. Le séquenceur APU fait avancer ses horloges frame à chaque cycle CPU pour garder un état déterministe quelle que soit la taille des lots passés à `tick_cpu_cycles`. Les accès DMC provoquent maintenant un blocage nominal de quatre cycles par rechargement du buffer, et ces cycles supplémentaires avancent aussi le CPU, la PPU et l'APU. L'API de session intégrée est exposée et testée dans ce dépôt, mais le branchement compilable au contrat hôte et la compatibilité des jeux ne sont pas encore établis.

## Langage et intégration

- **Langage :** Rust, en cohérence avec PeaceEmu.
- **Édition cible :** Rust 2021 ; respecter la version minimale et les conventions du dépôt hôte lors de l'intégration.
- **Responsabilité de ce dépôt :** matériel émulé et tests du cœur NES.
- **Responsabilité du dépôt PeaceEmu :** interface, bibliothèque, fenêtres, cycle de vie des sessions et services communs.
- **Contrat d'intégration :** adapter `NesSession` pour implémenter `EmulationCore`, fournir un identifiant système stable, déclarer les capacités réellement prises en charge et convertir les images/audio vers les types communs.
- **Point à résoudre avant le branchement compilable :** le crate emulation-core se trouve actuellement dans le workspace du dépôt hôte et n'est pas publié comme dépendance. Il faut choisir une stratégie de partage/versionnement du contrat au lieu de dupliquer ses types.

## Réutilisation de références existantes

Le développement s'appuie sur des émulateurs NES existants et des références matérielles plutôt que de réécrire chaque détail isolément. Le CPU intègre maintenant des variantes non officielles LAX/SAX/LAS, les opérations immédiates ANC/ALR/ARR/AXS/XAA (XAA reste un modèle instable approximatif) et les stores instables AHX/TAS/SHX/SHY avec corruption d’adresse au franchissement de page modélisée. Ces comportements sont adaptés à nos registres, à notre bus et à nos tests, sans prétendre reproduire tous les cas électriques instables du 2A03. Les projets étudiés, leurs licences, leurs limites de compatibilité et la méthode de portage sont documentés dans [REFERENCE_IMPLEMENTATIONS.md](docs/REFERENCE_IMPLEMENTATIONS.md). Cette démarche privilégie des ports ciblés et testés : aucune dépendance d'émulateur complète n'est ajoutée aveuglément, et les composants PeaceEmu gardent leurs propres interfaces.

## Implémentation actuelle

- **CPU :** sous-ensemble élargi des instructions officielles : chargements/stores, ADC/SBC, AND/ORA/EOR, comparaisons, BIT, shifts/rotates, incréments/décréments, branchements, JMP indirect avec comportement de page 6502, pile, JSR/RTS, RTI et BRK. Plusieurs opcodes non officiels stables sont aussi reconnus : LAX/SAX/LAS, ANC, ALR, ARR, AXS, AHX/TAS/SHX/SHY (modèle des stores instables et du masquage de page), les opérations composites SLO/RLA/SRE/RRA/DCP/ISC et des NOP avec lectures mémoire, opérandes et pénalités de franchissement de page ; les opcodes encore inconnus renvoient une erreur explicite. Les opérations read-modify-write officielles et composites reproduisent maintenant les écritures successives de l'ancienne puis de la nouvelle valeur ; les formes indexées incluent aussi leurs lectures factices de bus. Ce n'est pas encore un CPU complet ni cycle-exact.
- **Bus :** 2 Kio de RAM interne avec miroirs et latch open-bus minimal ; la zone cartouche `$6000–$7FFF` route vers les 8 Kio de PRG-RAM NROM, accessible en lecture/écriture ; registres PPU ($2000–$3FFF), DMA OAM ($4014), APU ($4000–$4017 hors adresses réservées), compteurs de longueur, séquenceur de frame approximatif, premiers registres DMC/lecture d'échantillon et deux manettes série ($4016/$4017) routés. Les manettes exposent huit boutons par port, latch sur strobe et décalage sériel avec ligne haute après le huitième bit.
- **PPU :** première couche matérielle : registres CPU, adresse VRAM, buffer de lecture, mémoire nametable/palette, OAM, mirroring de cartouche et drapeau VBlank/compteur de scanline. Le DMA OAM copie 256 octets d'une page CPU vers l'OAM et ajoute 513/514 cycles de blocage selon l'alignement. Un rendu par instantané est disponible via `NesMachine::render_background()` ou `NesMachine::render_frame()` et `NesMachine::framebuffer()` : le framebuffer de 256×240 pixels contient des indices de palette NES 6 bits. Le rendu lit les tuiles, plans de bits, attributs de palette, mirroring et défilement de base ; `render_frame()` compose aussi les sprites avec flips horizontal/vertical, tailles 8×8/8×16, priorité approximative, hit sprite 0 et overflow de sprites par ligne. Le rendu snapshot recalcule les indicateurs hit/overflow à chaque rendu afin d’éviter les résultats périmés entre deux snapshots. Ce n'est pas encore une pipeline dot-par-dot. Le NMI VBlank est routé vers l'entrée d'interruption CPU via une ligne NMI à détection de front montant (pas de retrigger lors d'écritures répétées de PPUCTRL tant que la ligne reste haute) ; l'IRQ frame APU est aussi exposée comme ligne niveau et prise en compte aux frontières d'instructions quand I=0 ; le CPU donne la priorité au NMI et conserve l'IRQ APU en attente ; le compteur NTSC modélise aussi le saut de dot du pré-rendu lors des images impaires avec rendu actif.
- **Cartouche :** parsing iNES legacy et dispatch statique pour NROM (mapper 0), MMC1 (mapper 1), UxROM (mapper 2), CNROM (mapper 3), MMC3 (mapper 4), MMC2 (mapper 9), AxROM (mapper 7), CPROM (mapper 13), GxROM (mapper 66), Color Dreams (mapper 11), Camerica (mapper 71) et BNROM (mapper 34). MMC3 fournit les fenêtres PRG de 8 Kio, les banques CHR de 1/2 Kio, la protection PRG-RAM, le mirroring commutable et les registres d’IRQ scanline (horloge encore approximée par ligne). GxROM commute indépendamment les fenêtres PRG de 32 Kio et CHR de 8 Kio, avec limites de banques et mirroring iNES fixe. MMC1 prend en charge le registre série cinq bits, les modes PRG 16/32 Kio, les banques CHR 4 Kio/8 Kio, le désactivation de PRG-RAM par registre, le mirroring contrôlé (one-screen/vertical/horizontal) et CHR-RAM quand aucune CHR-ROM n'est déclarée. MMC2 (mapper 9) fournit une fenêtre PRG de 8 Kio commutable, trois fenêtres fixes, quatre banques CHR de 4 Kio sélectionnées par deux paires de registres à latch FD/FE et mirroring commutable. Les mappers ont des tests de banque, limites, mirroring et écritures. Les trainers iNES sont copiés en PRG-RAM à `$7000–$71FF`. Les variantes à bus conflicts, NES 2.0 et les autres mappers ne sont pas encore pris en charge.
- **Machine :** `NesMachine` charge les cartouches iNES supportées, initialise le CPU depuis le vecteur de reset et expose le pas d'instruction ainsi que les lectures/écritures CPU. Le chargement et la commutation de banques MMC1 sont testés de bout en bout via le bus CPU.
- **Tests :** la suite couvre aussi les opcodes non officiels LAX/SAX, ANC, ALR, ARR, AXS et les NOP (modes immédiat, zéro-page, absolu et absolu,X avec franchissement de page), les stores instables AHX/TAS/SHX/SHY, dont le masquage de données, la corruption d’adresse en cas de franchissement de page et le changement de SP par TAS, la PRG-RAM cartouche, le chargement du trainer à `$7000`, le rejet des tailles CHR incompatibles avec NROM, et l'accès à la PRG-RAM via le bus CPU, en plus des régressions CPU/APU/PPU/manettes précédentes.
- **CI :** workflow GitHub Actions exécute formatage, tests et Clippy. Le statut doit être confirmé sur le commit final du lot.

## Ordre de développement

1. Étendre le rendu PPU vers une pipeline par dot. Les incréments coarse X/fine Y, le transfert horizontal au dot 257 et le transfert vertical au pré-rendu sont maintenant déclenchés aux points de timing prévus quand le rendu est activé ; les fetchs de pixels restent snapshot-based, et les collisions/overflow ne sont pas encore évalués en temps réel.
2. Compléter le CPU : CMP dispose maintenant des modes indirect X, zéro-page et indirect Y avec pénalité de franchissement de page ; RTI restaure le statut et le PC depuis la pile. Les modes AND absolu,Y et indirect,Y appliquent aussi la pénalité de franchissement de page. Poursuivre les instructions officielles restantes et leurs cycles.
3. Raffiner l’APU : le DMC initial inclut maintenant lecture mémoire, buffer, unité delta, bouclage et IRQ, et son octet est servi à l’échelle du cycle APU. Les tests couvrent aussi l'arrêt du DMC, l'effacement des octets restants et l'acquittement IRQ par $4015 ; restent l'arbitrage/stall DMA cycle-exact, le filtrage, le resampling à fréquence hôte et un timing plus fidèle. Raffiner le timing DMA/NMI.
4. Intégrer le contrat hôte PeaceEmu sans dépendance graphique dans le cœur.
5. Ajouter sauvegardes et tests de compatibilité reproductibles.
6. Étendre progressivement les mappers à partir de références permissives et des spécifications NESdev ; Camerica (71) et Color Dreams (11) sont maintenant ajoutés dans des adaptations locales testées.

## Principes

- Ne pas recréer une interface graphique ou une bibliothèque de jeux dans ce dépôt.
- Garder le matériel NES indépendant de l'interface et du système d'exploitation.
- Ne pas confondre instructions CPU exécutées et jeu correctement émulé : le timing et les composants vidéo/audio comptent.
- Ajouter des tests ciblés avec chaque fonctionnalité ; ne pas supprimer de tests de régression pour faire passer la CI.
- Ne pas déclarer une capacité prise en charge avant son implémentation et ses tests.
- Ne pas ajouter de ROM commerciale au dépôt. Utiliser des homebrews ou des suites de tests dont la licence autorise l'usage.

## Avancement

**Avancement global estimé : 77 %.** La machine minimale charge une cartouche NROM, exécute un sous-ensemble du CPU, et la PPU expose les registres, la mémoire graphique, le timing VBlank/NMI, des incréments/copies de scroll synchronisés à certains dots, des fetchs de tuiles en fin de ligne (dots 328 et 336) et un rendu par instantané. Les indicateurs sprite hit/overflow sont réinitialisés et recalculés lors de chaque rendu snapshot ; pendant l'émulation normale, chaque ligne visible est construite au dot 256 et le drapeau sprite-0 hit devient observable avant la fin de la frame. Le scroll est capturé séparément au début de chaque ligne, et `step_frame()` ne remplace plus ce résultat par un snapshot final. `NesMachine::step_frame()` avance désormais jusqu'à la prochaine frontière de frame, rend une image et renvoie le nombre nominal de cycles CPU consommés ; le passage initial du pré-rendu est ignoré au démarrage, et un budget de sécurité dépassé renvoie une erreur explicite. Une suite de tests couvre les composants de base, le transfert DMA des 256 octets avec ses deux parités de blocage (513/514 cycles), la progression PPU à trois dots par cycle CPU, l'entrée NMI, le décodage des pixels, la priorité des sprites, le protocole série des manettes, les mirroring nametables/palettes et le saut de dot des images impaires, les incréments de scroll aux dots 256/257/328/336 et le recalcul de l'overflow sprite. L’APU expose un échantillon mono normalisé et sa ligne IRQ frame est désormais prise en compte par le CPU lorsque les interruptions sont autorisées ; le CPU empile le PC et le statut, puis charge le vecteur IRQ. L’APU modélise approximativement les enveloppes pulse, le sweep, le compteur linéaire du triangle et un premier canal DMC ; les modes 4/5 étapes et l’effacement de l’IRQ frame par inhibition ont des tests dédiés : registres $4010–$4013, adresse et longueur d'échantillon, buffer, sortie delta 7 bits, démarrage silencieux, bouclage et IRQ DMC. Le bus alimente le buffer depuis l'espace d'adressage CPU entre les cycles APU et les tests couvrent le chargement du buffer, le démarrage silencieux et le wrap d'adresse ; les rechargements du buffer DMC ajoutent quatre cycles de blocage au CPU et font avancer les horloges PPU/APU, mais l'arbitrage d'alignement exact reste approximatif ; le mixage et le timing restent approximatifs, avec resampling à fréquence hôte à faire. Des tests supplémentaires couvrent le masquage des huit pixels de gauche du fond, l'acquittement de l'IRQ frame en activant l'inhibition, le passage entre séquenceurs quatre/cinq étapes et le port PPUDATA (buffer et incrément de 1/32), le DAC DMC, le bouclage, le retour d'adresse et les transferts mémoire par le bus. La lecture de `$2002` teste aussi le reset du latch partagé, le nettoyage du VBlank au pré-rendu et la suppression du VBlank/NMI lorsqu'elle survient au dot précédant son activation ; le scroll de début de frame est rafraîchi même si le rendu est désactivé au pré-rendu. Des tests vérifient les fronts NMI et la priorité du NMI sur une IRQ APU en attente. Le timing reste approximatif et la compatibilité de jeux n’est pas encore établie. Ce pourcentage mesure l'avancement du développement, pas la compatibilité des jeux : aucune ROM commerciale ou homebrew complète n'est encore déclarée compatible.

Le dernier lot ajoute BNROM (mapper 34, variante BNROM uniquement), en plus de Camerica (mapper 71) et Color Dreams (mapper 11). BNROM commute une fenêtre PRG de 32 Kio et fournit 8 Kio de CHR-RAM ; CPROM ajoute 32 Kio de PRG fixe et quatre banques supérieures de CHR-RAM de 4 Kio (la zone CHR basse reste fixe) ; Camerica commute une banque PRG de 16 Kio avec banque finale fixe, tandis que Color Dreams commute PRG/CHR indépendamment. Des tests couvrent les banques, les limites, le mirroring et les écritures CHR. Le cœur passe de 10 à 11 familles de mappers, sans revendiquer la prise en charge de toutes les variantes de cartes. La correction PPU/MMC3 et l'opcode LAS restent couverts par les tests existants.

| Domaine | État |
|---|---|
| Structure Rust et build initial | En place |
| CPU Ricoh 2A03 | Partiel — sous-ensemble élargi, opcodes non officiels composites et stores instables ciblés ; écritures read-modify-write et lectures factices d'adressage indexé partiellement modélisées ; pile/flags, NMI et IRQ APU testés ; exécution cycle-exacte encore à faire |
| Bus RAM, DMA OAM et manettes | RAM miroir, registres PPU, transfert DMA/blocage CPU, deux ports série manette |
| Cartouches iNES legacy | NROM, MMC1, UxROM, CNROM, MMC3 (mapper 4), AxROM, GxROM (mapper 66), Camerica (mapper 71, sous-ensemble iNES/CHR-RAM), Color Dreams (mapper 11, banques PRG/CHR), BNROM (mapper 34, variante BNROM uniquement), CPROM (mapper 13), MMC2 (mapper 9) et UNROM-512 (mapper 30, sous-ensemble iNES) ; NES 2.0 et autres mappers à faire |
| Machine et reset vector | Première version — charge NROM, exécute le CPU, synchronise les dots PPU à 3:1 et gère les deux parités de stall DMA OAM |
| PPU | Registres/mémoire/VBlank-NMI ; scroll capturé par ligne, rendu des 240 lignes au fil de la frame et hit/overflow sprite partiellement temporels ; fetchs de pixels et effets de registre en milieu de ligne encore approximatifs |
| APU | Partiel — registres, compteurs, enveloppes pulse, sweep, compteur linéaire triangle et premier DMC (lecture mémoire, buffer, démarrage silencieux, sortie delta, bouclage/IRQ, arrêt/acquittement testés) ; arbitrage DMA, filtrage, resampling et timing exact à faire |
| Contrôleurs | Première version — ports série $4016/$4017, huit boutons et latch |
| Timing complet | À faire |
| API de session hôte | En place — chargement ROM, configuration audio, commandes manettes, framebuffer et PCM par frame, accès à la RAM batterie ; tests automatisés |
| Adaptateur EmulationCore | À faire — branchement au contrat partagé de PeaceEmu et conversion vers ses types vidéo/audio |
| Tests de compatibilité | À faire |


### Estimation de complétion

- **Cœur global : 79 %** — estimation d’ingénierie, pas un pourcentage de jeux compatibles ; les fetchs vidéo approximatifs et les cas CPU/APU non couverts restent des blocages majeurs.
- **CPU : 76 %** — lectures factices de branchements, d'adressage indexé et de NOP non officiels, écritures successives pour les instructions read-modify-write officielles et composites, ajout du modèle XAA ; l'exécution reste principalement au niveau instruction et non cycle par cycle.
- **PPU : 62 %** — rendu des lignes visible pendant la frame, scroll capturé à chaque ligne et indicateurs sprite-0 hit/overflow consultables avant la fin de frame ; fetchs pixel/dot et effets de registre en milieu de ligne encore approximatifs.
- **APU : 58 %** — canaux principaux, mixage approximatif, sortie PCM configurable et blocage CPU/horloges PPU lors des rechargements DMC ; filtrage et arbitrage DMA précis restent incomplets.
- **Cartouches et mappers : 97 %** — nombreuses familles iNES legacy et premiers cas NES 2.0 pris en charge ; variantes et plusieurs mappers restent absents.
- **Intégration / lancement jouable : 56 %** — fenêtre de jeu sans HUD ajouté, sortie audio PCM Windows, mise à l'échelle, taux audio et mapping clavier configurables au lancement, sauvegardes RAM batterie ; le lancement orchestré par PeaceEmu et le contrat EmulationCore partagé restent à raccorder.


UNROM-512 (mapper 30) prend en charge les images iNES legacy à 256/512 Kio de PRG-ROM et sans CHR-ROM : banque PRG de 16 Kio sélectionnable (bits 0–4), banque finale fixe, quatre banques CHR-RAM de 8 Kio (bits 5–6) et bit 7 de sélection one-screen lorsque l'en-tête indique cette configuration. La variante ajoutée n'émule pas le flash interne, les sous-mappers NES 2.0 ni les conflits de bus des cartes physiques ; elle n'expose pas de PRG-RAM, conformément à la fiche de carte documentée.
