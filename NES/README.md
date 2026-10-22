# PeaceEmu-NES — module NES

> Cœur d'émulation Nintendo Entertainment System (NES) pour l'application PeaceEmu existante.

## Objectif

Développer un cœur NES réel, indépendant de l'interface graphique, intégrable à l'application principale [PeaceEmu](https://github.com/Likounsee/PeaceEmu). L'interface, la bibliothèque de jeux, les fenêtres et l'orchestration des sessions existent déjà dans le dépôt principal : ce dépôt ne doit pas recréer ces fonctionnalités.

**État réel :** le crate Rust est créé. Le bus RAM, le parseur iNES/NROM, un sous-ensemble élargi du CPU, un modèle de registres APU, compteurs de longueur, séquenceur de frame approximatif et un mélangeur audio pulse/triangle/bruit/DMC à mixage non linéaire approximatif avec enveloppes, sweep et compteur linéaire triangle approximatifs, un premier canal DMC avec lecture d'échantillons par le bus et IRQ, ainsi que le rendu snapshot de la PPU disposent de code et de tests. Le module n'est pas encore déclaré jouable et n'est pas encore branché au contrat hôte.

## Langage et intégration

- **Langage :** Rust, en cohérence avec PeaceEmu.
- **Édition cible :** Rust 2021 ; respecter la version minimale et les conventions du dépôt hôte lors de l'intégration.
- **Responsabilité de ce dépôt :** matériel émulé et tests du cœur NES.
- **Responsabilité du dépôt PeaceEmu :** interface, bibliothèque, fenêtres, cycle de vie des sessions et services communs.
- **Contrat d'intégration :** adaptateur à écrire pour implémenter EmulationCore, fournir un identifiant système stable, déclarer les capacités réellement prises en charge et transférer images/audio via les types communs.
- **Point à résoudre avant le branchement compilable :** le crate emulation-core se trouve actuellement dans le workspace du dépôt hôte et n'est pas publié comme dépendance. Il faut choisir une stratégie de partage/versionnement du contrat au lieu de dupliquer ses types.

## Implémentation actuelle

- **CPU :** sous-ensemble élargi des instructions officielles : chargements/stores, ADC/SBC, AND/ORA/EOR, comparaisons, BIT, shifts/rotates, incréments/décréments, branchements, JMP indirect avec comportement de page 6502, pile, JSR/RTS, RTI et BRK. Les opcodes manquants renvoient une erreur explicite. Ce n'est pas encore un CPU complet ni cycle-exact.
- **Bus :** 2 Kio de RAM interne avec miroirs et latch open-bus minimal ; registres PPU ($2000–$3FFF), DMA OAM ($4014), APU ($4000–$4017 hors adresses réservées), compteurs de longueur, séquenceur de frame approximatif, premiers registres DMC/lecture d'échantillon et deux manettes série ($4016/$4017) routés. Les manettes exposent huit boutons par port, latch sur strobe et décalage sériel avec ligne haute après le huitième bit.
- **PPU :** première couche matérielle : registres CPU, adresse VRAM, buffer de lecture, mémoire nametable/palette, OAM, mirroring de cartouche et drapeau VBlank/compteur de scanline. Le DMA OAM copie 256 octets d'une page CPU vers l'OAM et ajoute 513/514 cycles de blocage selon l'alignement. Un rendu par instantané est disponible via `NesMachine::render_background()` ou `NesMachine::render_frame()` et `NesMachine::framebuffer()` : le framebuffer de 256×240 pixels contient des indices de palette NES 6 bits. Le rendu lit les tuiles, plans de bits, attributs de palette, mirroring et défilement de base ; `render_frame()` compose aussi les sprites avec flips horizontal/vertical, tailles 8×8/8×16, priorité approximative, hit sprite 0 et overflow de sprites par ligne. Ce n'est pas encore une pipeline dot-par-dot. Le NMI VBlank est routé vers l'entrée d'interruption CPU via une ligne NMI à détection de front montant (pas de retrigger lors d'écritures répétées de PPUCTRL tant que la ligne reste haute) ; l'IRQ frame APU est aussi exposée comme ligne niveau et prise en compte aux frontières d'instructions quand I=0 ; le CPU donne la priorité au NMI et conserve l'IRQ APU en attente ; le compteur NTSC modélise aussi le saut de dot du pré-rendu lors des images impaires avec rendu actif.
- **Cartouche :** parsing iNES legacy, validation des tailles, mirroring, PRG-ROM NROM (mapper 0), CHR-ROM ou CHR-RAM. NES 2.0 et les autres mappers ne sont pas pris en charge.
- **Machine :** `NesMachine` charge une image iNES/NROM, initialise le CPU depuis le vecteur de reset et expose le pas d'instruction ainsi que les lectures/écritures CPU.
- **Tests :** les tests couvrent les compteurs de longueur, la génération audio, les branches CPU avec franchissement de page, les ports manette, les sprites et le buffer/incrément PPUDATA, le sweep APU, la cohérence par lots de cycles et l'entrée IRQ CPU ; consulter la dernière CI avant d'affirmer son résultat.
- **CI :** workflow GitHub Actions ajouté pour formater les sources puis exécuter les tests et Clippy. Vérifier le dernier résultat dans GitHub Actions avant d'affirmer que la CI est verte.

## Ordre de développement

1. Étendre le rendu PPU vers une pipeline par dot, synchronisation scroll et précision du sprite 0/overflow.
2. Compléter le CPU : CMP dispose maintenant des modes indirect X, zéro-page et indirect Y avec pénalité de franchissement de page ; RTI restaure le statut et le PC depuis la pile. Les modes AND absolu,Y et indirect,Y appliquent aussi la pénalité de franchissement de page. Poursuivre les instructions officielles restantes et leurs cycles.
3. Raffiner l’APU : le DMC initial inclut maintenant lecture mémoire, buffer, unité delta, bouclage et IRQ, et son octet est servi à l’échelle du cycle APU ; restent l’arbitrage/stall DMA cycle-exact, le filtrage, le resampling à fréquence hôte et un timing plus fidèle. Raffiner le timing DMA/NMI.
4. Intégrer le contrat hôte PeaceEmu sans dépendance graphique dans le cœur.
5. Ajouter sauvegardes et tests de compatibilité reproductibles.
6. Étendre progressivement les mappers.

## Principes

- Ne pas recréer une interface graphique ou une bibliothèque de jeux dans ce dépôt.
- Garder le matériel NES indépendant de l'interface et du système d'exploitation.
- Ne pas confondre instructions CPU exécutées et jeu correctement émulé : le timing et les composants vidéo/audio comptent.
- Ajouter des tests ciblés avec chaque fonctionnalité ; ne pas supprimer de tests de régression pour faire passer la CI.
- Ne pas déclarer une capacité prise en charge avant son implémentation et ses tests.
- Ne pas ajouter de ROM commerciale au dépôt. Utiliser des homebrews ou des suites de tests dont la licence autorise l'usage.

## Avancement

**Avancement global estimé : 51 %.** La machine minimale charge une cartouche NROM, exécute un sous-ensemble du CPU, et la PPU expose les registres, la mémoire graphique, le timing VBlank/NMI ainsi qu'un rendu par instantané. `NesMachine::step_frame()` avance désormais jusqu'à la prochaine frontière de frame, rend une image et renvoie le nombre nominal de cycles CPU consommés ; le passage initial du pré-rendu est ignoré au démarrage, et un budget de sécurité dépassé renvoie une erreur explicite. Une suite de tests couvre les composants de base, le transfert DMA des 256 octets, l'entrée NMI, le décodage des pixels, la priorité des sprites, le protocole série des manettes, les mirroring nametables/palettes et le saut de dot des images impaires. L’APU expose un échantillon mono normalisé et sa ligne IRQ frame est désormais prise en compte par le CPU lorsque les interruptions sont autorisées ; le CPU empile le PC et le statut, puis charge le vecteur IRQ. L’APU modélise approximativement les enveloppes pulse, le sweep, le compteur linéaire du triangle et un premier canal DMC : registres $4010–$4013, adresse et longueur d'échantillon, buffer, sortie delta 7 bits, démarrage silencieux, bouclage et IRQ DMC. Le bus alimente le buffer depuis l'espace d'adressage CPU entre les cycles APU et les tests couvrent le chargement du buffer, le démarrage silencieux et le wrap d'adresse ; les lectures DMC n'appliquent pas encore de cycles de blocage/arbitrage précis ; le mixage et le timing restent approximatifs, avec resampling à fréquence hôte à faire. Des tests supplémentaires couvrent le port PPUDATA (buffer et incrément de 1/32), le DAC DMC, le bouclage, le retour d'adresse et les transferts mémoire par le bus. La lecture de `$2002` teste aussi le reset du latch partagé et le nettoyage du VBlank au pré-rendu ; un test vérifie les fronts NMI et la priorité du NMI sur une IRQ APU en attente. Le timing reste approximatif et la compatibilité de jeux n’est pas encore établie. Ce pourcentage mesure l'avancement du développement, pas la compatibilité des jeux : aucune ROM commerciale ou homebrew complète n'est encore déclarée compatible.

CI : vérifier l'exécution liée au commit le plus récent avant de considérer ce lot validé.

| Domaine | État |
|---|---|
| Structure Rust et build initial | En place |
| CPU Ricoh 2A03 | Partiel — sous-ensemble élargi, pile/flags/transferts/branches ; entrée NMI et IRQ masquable APU implémentées |
| Bus RAM, DMA OAM et manettes | RAM miroir, registres PPU, transfert DMA/blocage CPU, deux ports série manette |
| Cartouche NROM / iNES legacy | Première version |
| Machine et reset vector | Première version — charge NROM et exécute le CPU |
| PPU | Registres/mémoire/VBlank-NMI ; rendu de fond et sprites par instantané ; timing dot-par-dot à faire |
| APU | Partiel — registres, compteurs, enveloppes pulse, sweep, compteur linéaire triangle et premier DMC (lecture mémoire, buffer, démarrage silencieux, sortie delta, bouclage/IRQ) ; arbitrage DMA, filtrage, resampling et timing exact à faire |
| Contrôleurs | Première version — ports série $4016/$4017, huit boutons et latch |
| Timing complet | À faire |
| Adaptateur EmulationCore | À faire — partage du contrat à définir |
| Tests de compatibilité | À faire |
