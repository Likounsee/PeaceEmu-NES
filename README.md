# PeaceEmu-NES

Module NES pour l'écosystème PeaceEmu.

Le développement du cœur d'émulation se trouve dans le dossier [NES/](NES/). Consultez son [README](NES/README.md) pour la feuille de route et [l'architecture](NES/docs/ARCHITECTURE.md) pour les responsabilités des composants.

## État du projet

Le dépôt contient actuellement la documentation et la structure initiale du module. Le CPU, le bus, la cartouche, la PPU, l'APU et la synchronisation ne sont pas encore implémentés.

## Démarrage du développement

Le langage et le système de build restent à confirmer avant de créer le cœur exécutable. La première étape sera de fixer ces choix, puis d'implémenter le CPU avec des tests unitaires avant d'ajouter le bus et le support NROM (mapper 0).

Aucune ROM commerciale ne doit être ajoutée au dépôt.
