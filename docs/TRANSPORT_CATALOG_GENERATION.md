# AzerothCore transport topology manifest

Run `python3 tools/generate_transport_catalog.py` from the `wow-bot` directory.
The command reads the AzerothCore reference SQL files and writes
`transports.generated.json`. This output is a source topology manifest. The
runtime catalog in `transports.json` stays separately authored and keeps its
grounded `boarding` and `exit` positions.

[`transports.topology.generated.json`](../transports.topology.generated.json)
contains the 20 transport topologies generated from the local WotLK
`TaxiPathNode.dbc` data. Regenerate it with:

```sh
python3 tools/generate_transport_catalog.py \
  --taxipathnode-dbc /path/to/dbc/TaxiPathNode.dbc \
  --output transports.topology.generated.json
```

The reference `transports.sql` provides transport entries and names.
`gameobject_template.sql` provides each transport's `Data0` taxi path ID.
The checked-in `taxipathnode_dbc.sql` contains its table schema but no node
rows. Without taxi path nodes, the command reports that route topology is
unavailable for each transport. The path IDs still appear in the manifest.

To generate stops and directed map legs, pass a populated taxi path SQL dump or
the local WotLK DBC file:

```sh
python3 tools/generate_transport_catalog.py \
  --taxipathnode-dbc /path/to/dbc/TaxiPathNode.dbc
```

Or use `--taxipathnode-sql /path/to/taxipathnode_dbc.sql`. The generator reads
AzerothCore's `actionFlag == 2` stop frames, their map IDs, and node indices.
It uses consecutive stop frames as legs and includes the final-to-first loop
leg when the route has at least two stops. It also lists map transitions from
path teleports or map changes. It does not guess a stop from a map boundary.

The manifest marks legs with `requires_ground_points: true`. It does not
contain boarding or exit positions and is not accepted by the runtime. Add
those positions to the authored `transports.json` and validate them against
the installed navigation data. Do not copy the moving transport node
coordinates as passenger ground positions.
