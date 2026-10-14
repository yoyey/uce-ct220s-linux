# Protocole CT220S (relevé dans le logiciel constructeur PC_CT22X)

Le transport TCP du simulateur reprend exactement les rapports USB HID.

## Viewer -> boîtier : rapport de 65 octets

`00` (Report ID), commande, paramètres, puis des `00`.

| Commande | Paramètre | Rôle |
|---|---|---|
| `FF` | 0 Single, 1 Dual, 2 OPC | mode |
| `FB` | 0 = 10 K, 1 = 1 K, 2 = 47 R | résistance source |
| `FC` | 0..3 | bouton de fréquence |
| `FD` | 0 = 2.5 V, 1 = 5 V, 2 = 10 V, 3 = 20 V, 4 = 1.0 V | calibre |
| `FE` | 0 / 1 | balayage tension |
| `F4` | 0 / 1 | balayage fréquence |
| `F9` | masque, bit à 1 = tension exclue | valeurs du balayage tension |
| `F5` | masque, bit à 1 = fréquence exclue | valeurs du balayage fréquence |
| `F8` | % | tolérance mémorisée |
| `F7` | 4 index dans la table de 20 fréquences | affectation des boutons |
| `F6` | 01 | acquittement surtension |
| `FA` | 01 | fermeture du logiciel |

## Boîtier -> viewer : rapports de 64 octets utiles

1. Configuration `F0 FF` ; avec `data[k]` = octet k du rapport Windows de 65
   octets : 3 canal, 4 mode, 5 balayage, 6 calibre, 7 masque tension,
   8 fréquence, 9 résistance, 10 mode bis, 11 tolérance, 12 masque fréquence,
   13 balayage fréquence, 47-48 résistance de sortie, 49 n° matériel,
   50 surtension, 51 arrêt, 54-55 états OPC, 56 firmware, 57 bits ADC,
   58 rapports par courbe, 59 produit (220 / 221), 60-63 n° de série, 64 contrôle.
2. `packets` rapports de données (32 par défaut) : 16 couples
   `(courant u16 LE, tension u16 LE)`, points ADC 12 bits centrés sur 2048.

Un en-tête avec seulement `F0 FF canal` (n° matériel à 0) reste accepté :
32 rapports par courbe, sans synchronisation de l'interface.
