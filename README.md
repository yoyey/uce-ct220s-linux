# CT220S Viewer

Version avec :
- interface identique en mode USB et en mode fichier ;
- boutons de fréquence/résistance/mode/tension actifs en simulation fichier ;
- fréquence et résistance utilisées immédiatement pour recalculer R/C/L/Ph ;
- sélection visuelle des paramètres actifs ;
- détection d'un court-circuit avec affichage R = 0 Ω (seuil provisoire 1 Ω) ;
- calcul R/C/L/phase intégré.

En mode fichier, aucune commande HID n'est envoyée : les boutons pilotent seulement l'état de simulation et les calculs.

## Interface CH0 / CH1 / DUAL
- CH0 : courbe et mesures en orange, mesures en haut à gauche.
- CH1 : courbe et mesures en bleu, mesures en haut à droite.
- DUAL : superposition des deux courbes, mesures CH0 à gauche et CH1 à droite.
- La composante résistive série est calculée pour C/L afin d'afficher l'ESR / résistance série lorsqu'elle est significative.

## Interface style CT220S
Cette version conserve le fonctionnement USB/fichier et les calculs existants, mais réorganise uniquement l'interface : graphe noir à gauche, commandes groupées à droite et barre d'état en bas.

## Simulateur TCP du boîtier

Un simulateur graphique est fourni dans `simulator/`.

Lancer le viewer en écoute sur le port 3000 :

```bash
cargo run -- --tcp
```

Puis lancer le simulateur dans un autre terminal :

```bash
cargo run --manifest-path simulator/Cargo.toml
```

Le simulateur peut générer indépendamment sur CH0 et CH1 des courbes R, L, C, diode et Zener.
