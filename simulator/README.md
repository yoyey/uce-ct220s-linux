# Simulateur CT220S TCP

Ce dossier contient un simulateur indépendant du boîtier CT220S.

## Principe

Le viewer principal écoute en TCP sur `127.0.0.1:3000` et le simulateur s'y connecte.
Le simulateur génère exactement le format de courbe attendu par le viewer :

- 1 rapport de 64 octets : `F0 FF <canal> ...`
- 32 rapports de 64 octets
- 512 points, chaque point = `courant u16 LE` + `tension u16 LE`

## Lancer le viewer en mode simulateur

Depuis la racine du projet :

```bash
cargo run -- --tcp
```

ou :

```bash
cargo run -- --tcp 127.0.0.1:3000
```

## Lancer le simulateur

Dans un second terminal :

```bash
cargo run --manifest-path simulator/Cargo.toml
```

Puis cliquer sur **Connecter** et **Démarrer**.

## Composants simulés

Chaque canal CH0 / CH1 peut être configuré indépendamment :

- R : résistance
- L : inductance + résistance série
- C : capacité + ESR
- D : diode
- Dz : diode Zener

Le viewer peut piloter directement la configuration du simulateur avec les mêmes rapports que le boîtier réel : FA (mode), FB (résistance source), FC (fréquence) et FD (tension). Le simulateur met alors automatiquement à jour ses valeurs de configuration. Aucun protocole spécifique au simulateur n'est ajouté.
