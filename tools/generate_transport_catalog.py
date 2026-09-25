#!/usr/bin/env python3
"""Build an AzerothCore transport topology manifest.

This tool reports source route topology. It does not create grounded positions.
"""

import argparse
import hashlib
import json
import re
import struct
import sys
from pathlib import Path


def insert_rows(path, table):
    """Read tuples from MySQL INSERT statements without evaluating SQL."""
    if not path:
        return []
    body = Path(path).read_text(encoding="utf-8")
    rows = []
    for match in re.finditer(
        rf"INSERT\s+INTO\s+`?{re.escape(table)}`?\s+(?:\([^;]*?\)\s+)?VALUES\s*(.*?);",
        body,
        re.IGNORECASE | re.DOTALL,
    ):
        text = match.group(1)
        i = 0
        while i < len(text):
            if text[i] != "(":
                i += 1
                continue
            i += 1
            row, value, quoted = [], [], False
            while i < len(text):
                char = text[i]
                if quoted:
                    if char == "\\" and i + 1 < len(text):
                        value.append(text[i + 1])
                        i += 2
                        continue
                    if char == "'":
                        if i + 1 < len(text) and text[i + 1] == "'":
                            value.append("'")
                            i += 2
                            continue
                        quoted = False
                    else:
                        value.append(char)
                elif char == "'":
                    quoted = True
                elif char == ",":
                    row.append("".join(value).strip())
                    value = []
                elif char == ")":
                    row.append("".join(value).strip())
                    rows.append(row)
                    i += 1
                    break
                else:
                    value.append(char)
                i += 1
    return rows


def read_taxipath_sql(path):
    nodes = []
    for row in insert_rows(path, "taxipathnode_dbc"):
        if len(row) < 11:
            raise ValueError(f"taxipathnode row has {len(row)} fields; expected 11")
        nodes.append((int(row[1]), int(row[2]), int(row[3]), int(row[7])))
    return nodes


def read_taxipath_dbc(path):
    body = Path(path).read_bytes()
    if len(body) < 20 or body[:4] != b"WDBC":
        raise ValueError(f"{path} is not a WDBC file")
    count, fields, record_size, string_size = struct.unpack_from("<4I", body, 4)
    if record_size < 44 or len(body) < 20 + count * record_size + string_size:
        raise ValueError(f"{path} has an unsupported TaxiPathNode.dbc layout")
    nodes = []
    for index in range(count):
        offset = 20 + index * record_size
        row = struct.unpack_from("<11I", body, offset)
        nodes.append((row[1], row[2], row[3], row[7]))
    return nodes


def create_manifest(transport_rows, gameobject_rows, nodes):
    objects = {}
    for row in gameobject_rows:
        if len(row) >= 10 and row[1] == "15":
            # TransportMgr::GeneratePath reads GameObjectTemplate::moTransport.taxiPathId,
            # which is Data0 (column index 8 in the SQL row).
            objects[int(row[0])] = int(row[8])
    node_paths = {}
    for path_id, index, map_id, flags in nodes:
        node_paths.setdefault(path_id, []).append((index, map_id, flags))

    transports = []
    node_gaps = []
    for row in transport_rows:
        if len(row) < 3:
            raise ValueError("transports row must contain guid, entry, and name")
        entry, name = int(row[1]), row[2]
        path_id = objects.get(entry)
        route_nodes = sorted(node_paths.get(path_id, [])) if path_id is not None else []
        route = {
            "entry": entry,
            "name": name,
            "path_id": path_id,
            "nodes_available": bool(route_nodes),
            "legs": [],
        }
        if not route_nodes:
            node_gaps.append(entry)
        else:
            # AzerothCore marks transport stops with TaxiPathNode actionFlag == 2.
            # Map changes are retained as transitions, but do not imply a stop.
            path_nodes = [{"node": index, "map": map_id, "action_flag": flags}
                          for index, map_id, flags in route_nodes]
            stops = [node for node in path_nodes if node["action_flag"] == 2]
            for stop, node in enumerate(stops, start=1):
                node["stop"] = stop
            route["stops"] = stops
            route["map_transitions"] = [
                {"node": right["node"], "from_map": left["map"], "to_map": right["map"]}
                for left, right in zip(path_nodes, path_nodes[1:]) if left["map"] != right["map"]
            ]
            route["legs"] = [
                {
                    "from_stop": left["stop"], "to_stop": right["stop"],
                    "from_map": left["map"], "to_map": right["map"],
                    "from_node": left["node"], "to_node": right["node"],
                    "requires_ground_points": True,
                }
                for left, right in zip(stops, stops[1:])
            ]
            if len(stops) > 1:
                # Motion transports repeat their path. Include the final-to-first
                # directed leg so the topology represents both travel directions.
                left, right = stops[-1], stops[0]
                route["legs"].append({
                    "from_stop": left["stop"], "to_stop": right["stop"],
                    "from_map": left["map"], "to_map": right["map"],
                    "from_node": left["node"], "to_node": right["node"],
                    "wraps_path": True, "requires_ground_points": True,
                })
            route["loops"] = len(stops) > 1
        transports.append(route)
    return {
        "schema_version": 1,
        "source": "AzerothCore transports.sql and gameobject_template; TaxiPathNode data when available",
        "note": "Topology only. Boarding and exit positions must be authored and validated separately.",
        "transports": transports,
    }, node_gaps


def main():
    repo = Path(__file__).resolve().parents[2]
    sql_dir = repo / "refs/azerothcore-wotlk-master-ref/data/sql/base/db_world"
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--transports-sql", type=Path, default=sql_dir / "transports.sql")
    parser.add_argument("--gameobject-template-sql", type=Path,
                        default=sql_dir / "gameobject_template.sql")
    parser.add_argument("--taxipathnode-sql", type=Path, help="Optional populated TaxiPathNode SQL dump")
    parser.add_argument("--taxipathnode-dbc", type=Path, help="Optional client TaxiPathNode.dbc file")
    parser.add_argument("--output", type=Path, default=Path("transports.generated.json"))
    args = parser.parse_args()
    if args.taxipathnode_sql and args.taxipathnode_dbc:
        parser.error("provide only one TaxiPathNode input")
    try:
        nodes = (read_taxipath_sql(args.taxipathnode_sql) if args.taxipathnode_sql else
                 read_taxipath_dbc(args.taxipathnode_dbc) if args.taxipathnode_dbc else [])
        manifest, gaps = create_manifest(
            insert_rows(args.transports_sql, "transports"),
            insert_rows(args.gameobject_template_sql, "gameobject_template"), nodes)
        node_source = args.taxipathnode_sql or args.taxipathnode_dbc
        manifest["taxipathnode_source"] = (
            {
                "file": node_source.name,
                "sha256": hashlib.sha256(node_source.read_bytes()).hexdigest(),
            }
            if node_source
            else None
        )
        args.output.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    except (OSError, ValueError, IndexError, struct.error) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    print(f"wrote {len(manifest['transports'])} transports to {args.output}")
    if gaps:
        print(f"route nodes unavailable for {len(gaps)} entries: " + ", ".join(map(str, gaps)))
        print("The reference SQL has no taxi path node rows; provide a populated SQL dump or TaxiPathNode.dbc.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
