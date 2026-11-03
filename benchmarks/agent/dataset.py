"""Deterministic AX task catalog, versioned independently from the runner.

No reference implementation is copied into an agent workspace. Function cases and
expected outputs stay in the controller, not in the prompt or workspace.
"""
VERSION = "ax-capability-2026.11.02.2"


def catalog():
    tasks = []

    def function(name, spec, cases, category="coding_terminal"):
        tasks.append(dict(id=name, category=category, prompt=f"Implement solve(value) in solution.py. {spec} Use Python standard library. Verify your implementation before finishing.",
                          files={"solution.py": "def solve(value):\n    raise NotImplementedError\n"},
                          checks=[dict(kind="function", cases=cases)]))

    function("stable_unique", "Return unique list elements preserving first occurrence; input integers.", [([], []), ([3, 1, 3, 2, 1], [3, 1, 2]), ([0, -1, 0], [0, -1])])
    function("merge_intervals", "Merge overlapping OR touching closed integer intervals; return sorted intervals. Input may be unsorted.", [([], []), ([[5, 8], [1, 3], [3, 6]], [[1, 8]]), ([[9, 10], [1, 2]], [[1, 2], [9, 10]])])
    function("balanced_brackets", "Return whether (), [] and {} are balanced. Ignore other characters.", [("", True), ("a{b[c](d)}", True), ("([)]", False), ("]", False)])
    function("run_length_encode", "Encode consecutive characters as [[character,count],...]. Empty string returns [].", [("", []), ("aaabbcaa", [["a", 3], ["b", 2], ["c", 1], ["a", 2]]), ("你你a", [["你", 2], ["a", 1]])])
    function("flatten_lists", "Flatten arbitrarily nested JSON lists into a flat list, preserving non-list values including null.", [([], []), ([1, [2, [], [3]], None], [1, 2, 3, None]), ([[["x"]]], ["x"])])
    function("deep_merge", "Input [left,right] objects: recursively merge dictionaries; other right values replace left. Do not mutate inputs.", [([{"a": {"b": 1}}, {"a": {"c": 2}}], {"a": {"b": 1, "c": 2}}), ([{"a": [1]}, {"a": None}], {"a": None})])
    function("topological_sort", "Input object node -> dependency list. Return lexicographically smallest topological ordering; include dependency-only nodes. Return null on cycle.", [({}, []), ({"c": ["b"], "b": ["a"]}, ["a", "b", "c"]), ({"a": ["b"], "b": ["a"]}, None), ({"b": [], "a": []}, ["a", "b"])])
    function("shortest_path", "Input {edges:[[a,b],...],start:s,end:t}. Directed unweighted graph. Return shortest distance or -1.", [({"edges": [], "start": "a", "end": "a"}, 0), ({"edges": [["a", "b"], ["b", "c"]], "start": "a", "end": "c"}, 2), ({"edges": [["a", "b"]], "start": "b", "end": "a"}, -1)])
    function("csv_roundtrip", "Input a CSV string with header. Parse with csv semantics and return list of dictionaries; preserve strings.", [('a,b\n"x,y",2\n', [{"a": "x,y", "b": "2"}]), ("a,b\n", []), ('a,b\n"x\ny",\n', [{"a": "x\ny", "b": ""}])])
    function("word_frequency", "Count Unicode whitespace-separated words case-insensitively using casefold; retain punctuation.", [("", {}), ("Straße STRASSE a A", {"strasse": 2, "a": 2}), ("你好\t你好", {"你好": 2})])
    function("moving_average", "Input {values:list,k:positive integer}. Return averages of complete windows, [] when fewer than k.", [({"values": [1, 2, 3, 4], "k": 2}, [1.5, 2.5, 3.5]), ({"values": [], "k": 2}, []), ({"values": [5], "k": 1}, [5])])
    function("binary_search", "Input {values:sorted list,target:number}. Return first matching index or -1.", [({"values": [1, 2, 2, 2, 4], "target": 2}, 1), ({"values": [], "target": 0}, -1), ({"values": [1, 2], "target": 3}, -1)])
    function("transpose", "Transpose a rectangular matrix; [] and [[],[]] return [].", [([], []), ([[], []], []), ([[1, 2, 3], [4, 5, 6]], [[1, 4], [2, 5], [3, 6]])])
    function("lru_cache", "Input {capacity:nonnegative int,ops:[[put,key,value]|[get,key],...]}. Return outputs only for gets; missing is -1. Access and writes update recency.", [({"capacity": 2, "ops": [["put", "a", 1], ["put", "b", 2], ["get", "a"], ["put", "c", 3], ["get", "b"], ["get", "a"]]}, [1, -1, 1]), ({"capacity": 0, "ops": [["put", "a", 1], ["get", "a"]]}, [-1])])
    function("version_sort", "Sort dotted nonnegative integer version strings numerically, treating missing trailing fields as zero; stable ties.", [(["1.10", "1.2", "1.0", "1"], ["1.0", "1", "1.2", "1.10"]), ([], [])])
    function("path_normalize", "Normalize absolute POSIX paths lexically: collapse / and ., resolve .. without going above root. No filesystem access.", [("/a//b/.././c/", "/a/c"), ("/../../", "/"), ("/a/...", "/a/...")])
    function("retry_schedule", "Input {base,cap,attempts}. Return delays min(cap,base*2**i) for i from 0 to attempts-1.", [({"base": 2, "cap": 7, "attempts": 5}, [2, 4, 7, 7, 7]), ({"base": 1, "cap": 5, "attempts": 0}, [])])
    function("json_pointer", "Input {document,pointer}. Resolve RFC6901 JSON Pointer with ~1 and ~0 decoding; valid pointers only, including root empty string.", [({"document": {"a/b": {"~": [7]}}, "pointer": "/a~1b/~0/0"}, 7), ({"document": [1], "pointer": ""}, [1])])
    function("duration_parse", "Parse concatenated nonnegative integer durations with h,m,s into seconds; allow empty string = 0.", [("1h2m3s", 3723), ("90m", 5400), ("", 0), ("0s", 0)])
    function("pagination", "Input {items,page,size}; page is 1-based and size positive. Return {items:page slice,total_pages:ceiling(total/size)}.", [({"items": [1, 2, 3], "page": 2, "size": 2}, {"items": [3], "total_pages": 2}), ({"items": [], "page": 1, "size": 5}, {"items": [], "total_pages": 0})])

    # File tasks exercise discovery, parsing, transformations and verification.
    file_tasks = [
        ("json_filter", "Read input.json and write the sorted names of enabled records to output.json.", {"input.json": '[{"name":"z","enabled":true},{"name":"a","enabled":true},{"name":"b","enabled":false}]'}, ["a", "z"]),
        ("csv_totals", "Sum integer amount by region from sales.csv; write an object to output.json.", {"sales.csv": "region,amount\neast,3\nwest,5\neast,-1\n"}, {"east": 2, "west": 5}),
        ("log_errors", "Extract unique ERROR messages in encounter order from app.log; strip the prefix 'ERROR '. Write list to output.json.", {"app.log": "INFO ready\nERROR disk\nERROR network\nERROR disk\n"}, ["disk", "network"]),
        ("config_overlay", "Recursively merge base.json then override.json; write output.json. Lists replace.", {"base.json": '{"db":{"host":"local","port":1},"x":[1]}', "override.json": '{"db":{"port":2},"x":[2]}'}, {"db": {"host": "local", "port": 2}, "x": [2]}),
        ("ndjson_summary", "Parse records.ndjson, skip malformed lines, sum v values and count valid objects. Write {count,sum} to output.json.", {"records.ndjson": '{"v":2}\nbad\n{"v":-1}\n'}, {"count": 2, "sum": 1}),
        ("dependency_inventory", "Read all JSON manifests recursively below packages/. Write sorted unique dependency names to output.json.", {"packages/a/package.json": '{"dependencies":{"x":"1","y":"2"}}', "packages/b/package.json": '{"dependencies":{"x":"3","z":"1"}}'}, ["x", "y", "z"]),
        ("unicode_inventory", "Read names.txt, strip lines, remove empty lines, casefold and deduplicate; sort into output.json.", {"names.txt": "Straße\nSTRASSE\n你好\n\n"}, ["strasse", "你好"]),
        ("checksum_manifest", "Compute SHA256 for each file directly in payload/. Write filename->hex digest in output.json.", {"payload/a.txt": "hello", "payload/b.txt": ""}, {"a.txt": "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824", "b.txt": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}),
        ("join_datasets", "Join users.json and orders.json by user id, sum amounts, include users with zero orders. Write name->total to output.json.", {"users.json": '[{"id":1,"name":"a"},{"id":2,"name":"b"}]', "orders.json": '[{"user":1,"amount":3},{"user":1,"amount":4}]'}, {"a": 7, "b": 0}),
        ("repair_json", "settings.json contains a trailing comma. Repair it preserving all values and write the parsed object to output.json.", {"settings.json": '{"port":8080,"label":"a,b",}'}, {"port": 8080, "label": "a,b"}),
    ]
    for name, prompt, files, expected in file_tasks:
        tasks.append(dict(id=name, category="tool_execution", prompt=prompt, files=files,
                          checks=[dict(kind="json", path="output.json", expected=expected)]))

    # Two production CLI processes, same session for resume, a new session for memory.
    for i, (key, value) in enumerate([("release_port", "7319"), ("artifact_name", "cobalt"), ("region_code", "ap-7"), ("build_marker", "raven-42"), ("schema_version", "v17")]):
        for cross in (False, True):
            name = ("memory_cross_" if cross else "session_resume_") + key
            tasks.append(dict(id=name, category="memory_context", files={},
                turns=[dict(prompt=f"Remember the project fact {key} = {value}. " + ("Persist it using the memory tool for future sessions." if cross else "Keep it available for our next turn.")),
                       dict(prompt=f"Recall the previously supplied {key}. Write just its value to answer.txt. Do not guess.", new_session=cross)],
                checks=[dict(kind="text", path="answer.txt", expected=value)]))

    recovery = [
        ("repair_syntax", "Fix calc.py syntax; solve returns value+1.", "def solve(value)\n    return value + 1\n", [(2, 3), (-1, 0)]),
        ("repair_off_by_one", "Fix calc.py: sum all integers from 1 through value inclusive.", "def solve(value):\n    return sum(range(value))\n", [(0, 0), (1, 1), (10, 55)]),
        ("repair_empty", "Fix calc.py to return largest element or null for an empty list.", "def solve(value):\n    return max(value)\n", [([], None), ([-3, -2], -2)]),
        ("repair_mutation", "Fix calc.py to return a sorted list without mutating input.", "def solve(value):\n    value.sort()\n    return value\n", [([3, 1, 2], [1, 2, 3])]),
        ("repair_division", "Fix calc.py: input [numerator,denominator], return true division, null if denominator=0.", "def solve(value):\n    return value[0] // value[1]\n", [([3, 2], 1.5), ([3, 0], None)]),
        ("repair_casefold", "Fix calc.py to compare input pair of Unicode strings case insensitively.", "def solve(value):\n    return value[0].lower() == value[1].lower()\n", [(["Straße", "STRASSE"], True), (["a", "b"], False)]),
        ("repair_shared_default", "Implement solve(list of lists): concatenate into a new list, no state retained between calls.", "cache=[]\ndef solve(value):\n    for row in value: cache.extend(row)\n    return cache\n", [([[1], [2]], [1, 2]), ([[3]], [3])]),
        ("repair_missing_key", "Fix calc.py: sum the 'count' fields of object list; missing count means 0.", "def solve(value):\n    return sum(x['count'] for x in value)\n", [([{}, {"count": 3}], 3), ([], 0)]),
        ("repair_duplicates", "Fix calc.py: return sorted values occurring more than once, each only once.", "def solve(value):\n    return sorted(value)\n", [([1, 2, 2, 3, 3, 3], [2, 3]), ([], [])]),
        ("repair_crlf", "Fix calc.py: split nonempty text lines, handle CRLF and final newline, preserve spaces within lines.", "def solve(value):\n    return value.split('\\n')\n", [("a\r\nb\r\n", ["a", "b"]), ("", []), ("a b\n", ["a b"])])]
    for name, prompt, source, cases in recovery:
        tasks.append(dict(id=name, category="reliability_repair", prompt=prompt+" Verify behavior, preserve the public API.", files={"calc.py": source},
                          checks=[dict(kind="function", path="calc.py", cases=cases)]))

    function("transaction_ledger", "Input list of {id,account,amount} events. Ignore duplicate ids. Return account balances; retain zero balances.", [([{"id": "1", "account": "a", "amount": 5}, {"id": "1", "account": "a", "amount": 5}, {"id": "2", "account": "a", "amount": -5}], {"a": 0}), ([], {})], "complex_multistep")
    function("dependency_layers", "Input node->dependencies object (all nodes declared, acyclic). Return parallel execution layers, each lexically sorted, dependencies strictly in earlier layers.", [({"a": [], "b": [], "c": ["a"], "d": ["c", "b"]}, [["a", "b"], ["c"], ["d"]]), ({}, [])], "complex_multistep")
    function("event_reconciliation", "Input unordered events {key,version,value}; retain highest version for each key (equal versions use last encountered). Return key->value.", [([{"key": "x", "version": 2, "value": "new"}, {"key": "x", "version": 1, "value": "old"}], {"x": "new"}), ([], {})], "complex_multistep")
    function("job_scheduler", "Input {jobs:[{id,duration}],workers:positive int}. In input order assign each job to the earliest-free worker (tie smallest index), initially all free at 0. Return [{id,worker,start,end}].", [({"jobs": [{"id": "a", "duration": 3}, {"id": "b", "duration": 1}, {"id": "c", "duration": 2}], "workers": 2}, [{"id": "a", "worker": 0, "start": 0, "end": 3}, {"id": "b", "worker": 1, "start": 0, "end": 1}, {"id": "c", "worker": 1, "start": 1, "end": 3}])], "complex_multistep")
    function("expression_parser", "Evaluate integer arithmetic strings with +,-,*, parentheses, whitespace, unary minus, precedence. Return integer. Do not use eval or exec.", [("2+3*4", 14), ("-(2+3)*4", -20), ("1--2", 3), ("((7))", 7)], "complex_multistep")
    tasks[-1]["forbidden_calls"] = ["eval", "exec"]
    for name, prompt, expected in [
        ("mcp_lookup", "Look up cobalt and write its value as a number to output.json.", 7319),
        ("mcp_chain", "Starting at cobalt, follow next links until null. Write the values in traversal order to output.json.", [7319, 42, 17]),
        ("mcp_aggregate", "Look up cobalt, raven and amber, and write the sum of their values to output.json.", 7378),
        ("mcp_select", "Look up cobalt, raven and amber. Write the key with the smallest value to output.json as a JSON string.", "amber"),
        ("mcp_projection", "Look up raven. Write an object with only its value and next fields to output.json.", {"value": 42, "next": "amber"}),
    ]:
        tasks.append(dict(id=name, category="tool_mcp", mcp=True, files={},
            prompt="Use the bench MCP record service. "+prompt,
            checks=[dict(kind="json", path="output.json", expected=expected)]))
    return tasks
