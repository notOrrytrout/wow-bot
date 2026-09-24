# AzerothCore World Knowledge

Run this command from the repository root to rebuild the catalog:

```sh
python3 tools/generate_world_knowledge.py /path/to/azerothcore --dbc-dir /path/to/client/dbc
```

The command writes `crates/wow-infra/data/world-knowledge/azerothcore-catalog.json`.
It combines the existing quest catalog with generated AzerothCore quest starters,
turn-ins, item sources, world services, profession trainers, gathering nodes,
and fishing item classifications. It also regenerates quest action rules from
quest text, SQL rows, and client DBC spell names.

Quest spell rules require a named spell in an action sentence and a grounded
quest target. Quest item-use rules require a named quest item, its use spell in
`item_template`, one quest creature objective, and a matching target name in
the use clause. A temporary creature can qualify without a static spawn row
when its objective name or a distinctive term from its template name appears
in the use clause. If the source data does not identify one target, the
generator records the candidate under `unresolved_quest_action_hints`; it does
not turn that candidate into a runtime action.

The source provenance stores SQL and DBC hashes. A Git commit is stored when
the AzerothCore input directory is a Git checkout. A source archive has no
commit value, but its input hashes remain available.

The runtime loader and catalog types live in `wow-infra::world_knowledge`.
Policy code uses typed quest, spawn, item-source, vendor, trainer, and gather
queries. It does not deserialize the JSON or depend on its field layout.

Static spawns only provide search locations. Runtime policy still requires a
live entity and authoritative item instance, and the engine still validates
the action before execution.
