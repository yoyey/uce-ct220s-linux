# Protocole simulateur CT220S

Le transport TCP ne modifie pas le protocole CT220S. Il sert uniquement à remplacer le lien USB/HID pendant les tests.

## Viewer -> simulateur

Le viewer envoie exactement le même rapport HID de 65 octets qu'au boîtier réel :

- octet 0 : Report ID = `00`
- octet 1 : commande
- octet 2 : index
- octets suivants : `00`

Commandes conservées :

- `FA xx` : mode
- `FB xx` : résistance source
- `FC xx` : fréquence
- `FD xx` : tension

Aucun nouveau code de commande n'est ajouté pour le simulateur.

## Simulateur -> viewer

Les courbes gardent le format déjà décodé :

1. rapport de 64 octets commençant par `F0 FF`, avec le canal en octet 2 ;
2. 32 rapports de 64 octets ;
3. soit 2048 octets = 512 couples `(I,V)` en `u16` little-endian.

TCP est full-duplex : le simulateur peut envoyer les courbes pendant que le viewer lui envoie les commandes de configuration.
