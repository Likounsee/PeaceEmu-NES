# Tests NES

## Stratégie

- **CPU :** tests par instruction, modes d'adressage, flags, branchements, interruptions et cycles.
- **Bus :** décodage d'adresses, mirroring RAM et accès aux registres mappés.
- **Cartouches :** en-têtes valides/invalides, tailles, NROM, mirroring et erreurs de fichier.
- **PPU/APU :** registres, timing et comportements reproductibles.
- **Intégration :** démarrage de ROM homebrew et suites de tests dont l'utilisation est autorisée.

Les tests devront être automatisables en CI. Ne pas marquer un composant comme terminé simplement parce qu'il compile.
