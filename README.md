# CT220S Viewer (v0.2.0)

Visualiseur V-I pour les testeurs UCE-CT220S / CT221L, en Rust (egui).
Cette version reprend les fonctions et les algorithmes du logiciel
constructeur PC_CT22X (C#), et corrige plusieurs erreurs de la v0.1.

## Lancement

```bash
cargo run                       # boîtier USB (reconnexion automatique)
cargo run -- --file capture.txt # rejoue une capture hexadécimale
cargo run -- --tcp              # attend le simulateur sur 127.0.0.1:3000
cargo run --manifest-path simulator/Cargo.toml   # simulateur
cargo run --features son        # active l'alerte sonore (rodio)
```

Sous Linux, l'option `son` demande `libasound2-dev` ; sous Windows rien de plus.
Tests : `cargo test` (27 tests) et `cargo test --manifest-path simulator/Cargo.toml` (4 tests).

## Fonctions

**Basic** : modes CH0 / CH1 / DUAL / OPC, calibres 1.0 / 2.5 / 5 / 10 / 20 V,
fréquences (4 boutons, table réglable parmi 20 valeurs avec « Régler… »),
résistances 47 R / 1 K / 10 K, balayage tension et fréquence avec choix des
valeurs incluses, vue XY ou XT, échelle fixe (0–4095, comme le constructeur)
ou auto, curseur tension / courant (clic gauche, clic droit pour effacer),
export image (png, jpg, bmp, gif) et export texte (512 V puis 512 I).
Les règles constructeur sont appliquées : 47 R interdit en 10 V et 20 V,
calibres verrouillés pendant un balayage, OPC indisponible en balayage.

**Mesures** : algorithme constructeur par défaut (min/max, THD sur 31
harmoniques lissées, valeurs efficaces, phase par tables sin/cos, R/C/L avec
la résistance de sortie calibrée du boîtier, mêmes seuils et formats
d'affichage). L'ancien calcul par DFT reste disponible (« DFT Rust »).
Un court-circuit affiche `R: 0 R (court-circuit)`.

**Comparaison** : « Différence d'impédance » constructeur (corrélation V et I +
rapports des valeurs efficaces). En DUAL, CH0 est comparé à CH1 ; en test
automatique, la mesure est comparée au point de référence. Le verdict est
rendu une fois par contact de sonde (armement sonde en l'air, 15 trames de
contact), avec cadre vert / rouge et alerte sonore.

**Advanced** : références rapides en mémoire, différence d'impédance et écart
RMS, enregistrement CSV (format V2 en points ADC ; V1 toujours lisible).

**Base** (mode base de données) : liste de points de test (ajout après la
sélection, nom auto-incrémenté, suppression, déplacement, navigation <<< / >>>),
fichiers `.uce` compatibles avec PC_CT22X, image de la carte avec repères
(clic pour placer le repère, résultats vert / rouge), test automatique
(passage au point suivant au relevé de sonde si conforme, comme le
constructeur ; le test démarre au point sélectionné) et rapport PDF
(page de garde, carte annotée, courbe et zoom de la carte pour chaque test).

**Debug** : état, min/max, valeurs efficaces, THD, harmoniques, configuration brute.

**About / barre d'état** : appareil, firmware, numéro de série, résistance de
sortie calibrée, connexion, images par seconde. Alertes : haute tension
(acquittée par `F6 01`), firmware ancien, boîtier non calibré.

## Corrections par rapport à la v0.1

- La commande de mode est `FF` (0 Single, 1 Dual, 2 OPC). La v0.1 envoyait
  `FA`, que le logiciel constructeur n'envoie qu'à la fermeture (`FA 01`).
- Bouton « Offset » supprimé : il envoyait `FB 00` (10 K) sans mettre à jour
  l'interface, ce qui faussait les calculs.
- USB : lecture avec délai de 50 ms (les commandes ne sont plus bloquées
  pendant la lecture d'une courbe) et reconnexion à chaud.
- TCP : socket accepté repassé en mode bloquant (sous Windows il héritait du
  mode non bloquant de l'écouteur) et resynchronisation à l'octet sur `F0 FF`
  au lieu de blocs de 64 octets qui pouvaient rester désalignés.
- La trame de configuration `F0 FF` est entièrement décodée (nombre de
  rapports par courbe, calibres, balayages, tolérance, résistance de sortie
  calibrée, numéro de série, firmware, surtension) et l'interface se cale sur
  l'état réel du boîtier.
- 47 R vaut 48,1 Ω plus la calibration, comme chez le constructeur.
- Calibre 1.0 V ajouté.

Écart volontaire : le logiciel constructeur réinitialise sa table de
fréquences à chaque trame, ce qui fausse ses calculs C/L après un réglage
`F7`. Ici les fréquences réglées sont conservées pour les calculs.

Non repris : vérification des mises à jour en ligne, géolocalisation IP,
conversion d'image `.pcb` (spécifique CT221L).

## Validation

Compilé et testé sous Linux avec Rust 1.91 : tests unitaires, puis essais de
bout en bout du viewer sur écran virtuel avec un faux boîtier TCP (synchro des
commandes, mesures R / C, DUAL et verdict, base de données, test
automatique sur deux points, rapport PDF avec image de carte, mode fichier,
curseur, XT, alarme surtension). Non testé : boîtier réel en USB et
compilation Windows.
