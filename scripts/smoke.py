#!/usr/bin/env python3
"""End-to-end check: start the app (in the background, on throwaway files) and drive every
feature area over MCP, asserting on what comes back. Exit code 0 = all passed.

usage: smoke.py [binary]   (default target/debug/rusty-beaver)
"""
import json, os, sqlite3, subprocess, sys, tempfile, time

sys.path.insert(0, os.path.dirname(__file__))
import mcp  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DB = "Smoke"
failures = []


def call(tool, args=None, ok=True):
    mcp.session = None
    mcp.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "smoke", "version": "1"}})
    mcp.rpc("notifications/initialized", notify=True)
    res = mcp.rpc("tools/call", {"name": tool, "arguments": args or {}})
    text = res["content"][0].get("text", "")
    if res.get("isError") == ok:
        raise AssertionError(f"{tool} {'failed' if ok else 'should have failed'}: {text[:300]}")
    try:
        return json.loads(text)
    except ValueError:
        return text


def check(name, fn):
    try:
        fn()
        print(f"  ok    {name}")
    except Exception as e:  # noqa: BLE001 - report every failure, keep going
        failures.append(name)
        print(f"  FAIL  {name}: {e}")


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "target", "debug", "rusty-beaver")
    tmp = tempfile.mkdtemp()
    db = os.path.join(tmp, "smoke.db")
    con = sqlite3.connect(db)
    con.executescript(open(os.path.join(ROOT, "src", "demo.sql")).read())
    con.close()
    cfg = os.path.join(tmp, "connections.json")
    json.dump([{"name": DB, "driver": "sqlite", "database": db}], open(cfg, "w"))
    port = os.environ.get("SMOKE_PORT", "7880")
    mcp.URL = f"http://127.0.0.1:{port}/mcp"
    env = dict(os.environ, RUSTY_BEAVER_CONFIG=cfg, RUSTY_BEAVER_MCP_PORT=port, RUSTY_BEAVER_BACKGROUND="1", RUSTY_BEAVER_KEYCHAIN="off",
               RUSTY_BEAVER_HISTORY=os.path.join(tmp, "h.jsonl"), RUSTY_BEAVER_SESSION=os.path.join(tmp, "s.json"))
    app = subprocess.Popen([binary], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(100):
            try:
                call("get_state")
                break
            except Exception:  # noqa: BLE001 - not up yet
                time.sleep(0.1)
        q = lambda sql: sqlite3.connect(db).execute(sql).fetchall()  # noqa: E731

        def navigator():
            tables = [t["name"] for t in call("connect", {"connection": DB})["tables"]]
            assert tables == ["beavers", "leaderboard", "lodges", "missions"], tables
            cols = call("describe_table", {"connection": DB, "table": "missions"})["rows"]
            assert ["leader_id", "INTEGER", "YES", "FK", "beavers.id"] in cols, cols

        def sql_editor():
            r = call("run_sql", {"connection": DB, "sql": "SELECT name FROM beavers ORDER BY logs_chewed DESC"})
            assert r["rows"][0] == ["Marco"] and r["row_count"] == 8
            r = call("run_sql", {"sql": "SELECT 1 AS a; SELECT 2 AS b;"})
            assert len(r["results"]) == 2 and r["rows"] == [["2"]]
            assert call("get_results", {"index": 0})["rows"] == [["1"]]
            assert "no such column" in call("run_sql", {"sql": "SELECT nope"}, ok=False)

        def params_format_explain():
            r = call("run_sql", {"sql": "SELECT count(*) FROM beavers WHERE rank = :rank", "params": {"rank": "'Private'"}})
            assert r["rows"] == [["2"]], r
            assert call("format_sql", {"sql": "select a from t where b = 1"})["sql"].startswith("SELECT\n    a\nFROM t")
            assert call("explain", {"sql": "SELECT * FROM beavers WHERE id = 1"})["row_count"] >= 1
            labels = [c["label"] for c in call("sql_completions", {"connection": DB, "sql": "SELECT b. FROM beavers b", "offset": 9})]
            assert "logs_chewed" in labels, labels

        def table_editing():
            assert call("open_table", {"connection": DB, "table": "lodges"})["row_count"] == 4
            call("edit_cell", {"row": 0, "column": "river", "value": "Yukon River"})
            call("add_row", {"values": {"name": "Smoke Lodge", "river": "Elbe"}})
            call("delete_row", {"row": 3})  # referenced by beavers: the foreign key refuses
            assert "FOREIGN KEY" in call("save_changes", ok=False)
            call("delete_row", {"row": 3})  # unmark it
            assert call("save_changes")["saved"]
            assert q("SELECT river FROM lodges WHERE id = 1") == [("Yukon River",)]
            assert q("SELECT count(*) FROM lodges") == [(5,)]
            assert call("set_filter", {"filter": "river = 'Elbe'"})["row_count"] == 1
            call("open_table", {"connection": DB, "table": "missions"})
            assert "beavers" in call("open_referenced_row", {"row": 0, "column": "leader_id"})["ok"]
            for _ in range(50):  # the referenced table loads in the background
                r = call("get_results")
                if r["row_count"] == 1:
                    break
                time.sleep(0.1)
            assert r["rows"][0][1] == "Marco", r
            call("open_table", {"connection": DB, "table": "lodges"})
            ddl = call("set_table_view", {"view": "ddl"})["ddl"]
            assert ddl.startswith("CREATE TABLE lodges"), ddl
            call("set_table_view", {"view": "data"})

        def transactions_and_production():
            call("set_auto_commit", {"connection": DB, "on": False})
            call("run_sql", {"connection": DB, "sql": "DELETE FROM missions"})
            call("rollback", {"connection": DB})
            assert q("SELECT count(*) FROM missions") == [(6,)]
            call("set_auto_commit", {"connection": DB, "on": True})
            call("edit_connection", {"connection": DB, "production": True})
            assert "confirm" in call("run_sql", {"connection": DB, "sql": "DELETE FROM missions WHERE id = 6"}, ok=False)
            call("run_sql", {"connection": DB, "sql": "DELETE FROM missions WHERE id = 6", "confirm": True})
            assert q("SELECT count(*) FROM missions") == [(5,)]
            call("edit_connection", {"connection": DB, "production": False, "read_only": True})
            assert "read-only" in call("run_sql", {"connection": DB, "sql": "DELETE FROM missions", "confirm": True}, ok=False)
            assert q("SELECT count(*) FROM missions") == [(5,)]
            call("edit_connection", {"connection": DB, "read_only": False})

        def cancel():
            r = {}

            def long_query():
                try:
                    call("run_sql", {"connection": DB, "sql": "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n) SELECT count(*) FROM n"}, ok=False)
                    r["ok"] = True
                except Exception as e:  # noqa: BLE001
                    r["err"] = e
            import threading
            th = threading.Thread(target=long_query)
            th.start()
            for _ in range(50):
                time.sleep(0.1)
                try:
                    call("cancel_query")
                    break
                except AssertionError:
                    pass
            th.join(10)
            assert r.get("ok"), r

        def io_and_views():
            out = os.path.join(tmp, "beavers.json")
            call("run_sql", {"connection": DB, "sql": "SELECT name, rank FROM beavers"})
            call("export_csv", {"path": out})
            assert json.load(open(out))[0]["name"] == "Marco"
            csv = os.path.join(tmp, "lodges.csv")
            open(csv, "w").write("name,river\nCSV Lodge,Rhine\n")
            assert call("import_csv", {"connection": DB, "table": "lodges", "path": csv})["imported"] == 1
            call("run_sql", {"connection": DB, "sql": "SELECT name, logs_chewed FROM beavers"})
            assert call("chart", {})["value_columns"] == ["logs_chewed"]
            call("chart", {"on": False})
            assert call("record_view", {"on": True, "row": 1})["record"]["name"]
            call("record_view", {"on": False})
            er = call("open_er_diagram", {"connection": DB})
            assert len(er["relations"]) == 2, er["relations"]
            assert call("generate_sql", {"connection": DB, "table": "lodges", "kind": "select"})["sql"].startswith("SELECT")
            assert len(call("get_history", {"limit": 3})) == 3
            png = call("screenshot")
            assert png is not None

        for name, fn in [("navigator + foreign keys", navigator), ("sql editor + scripts + errors", sql_editor),
                         ("parameters, format, explain, completion", params_format_explain), ("table editing, filter, ddl", table_editing),
                         ("manual commit, production + read-only safety", transactions_and_production), ("cancel", cancel),
                         ("export, import, views, er diagram, history", io_and_views)]:
            check(name, fn)
    finally:
        app.terminate()
    print(f"{len(failures)} failed" if failures else "all passed")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
