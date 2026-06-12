import type { SnippetDef } from './types';

export interface KeywordDef {
	label: string;
	detail?: string;
	kind: 'function' | 'keyword';
}

export function getPythonKeywords(): KeywordDef[] {
	const builtins: [string, string][] = [
		['print', 'print(value, ..., sep, end, file, flush)'],
		['len', 'len(s) -> int'],
		['range', 'range(stop) / range(start, stop, step)'],
		['type', 'type(object) -> type'],
		['int', 'int(x, base=10)'],
		['float', 'float(x)'],
		['str', 'str(object)'],
		['bool', 'bool(x)'],
		['list', 'list(iterable)'],
		['dict', 'dict(**kwargs)'],
		['set', 'set(iterable)'],
		['tuple', 'tuple(iterable)'],
		['input', 'input(prompt) -> str'],
		['open', 'open(file, mode, ...) -> file object'],
		['enumerate', 'enumerate(iterable, start=0)'],
		['zip', 'zip(*iterables)'],
		['map', 'map(function, iterable, ...)'],
		['filter', 'filter(function, iterable)'],
		['sorted', 'sorted(iterable, key, reverse)'],
		['reversed', 'reversed(seq)'],
		['abs', 'abs(x) -> number'],
		['min', 'min(iterable, key, default)'],
		['max', 'max(iterable, key, default)'],
		['sum', 'sum(iterable, start=0)'],
		['round', 'round(number, ndigits)'],
		['isinstance', 'isinstance(object, classinfo) -> bool'],
		['issubclass', 'issubclass(class, classinfo) -> bool'],
		['hasattr', 'hasattr(object, name) -> bool'],
		['getattr', 'getattr(object, name, default)'],
		['setattr', 'setattr(object, name, value)'],
		['delattr', 'delattr(object, name)'],
		['repr', 'repr(object) -> str'],
		['id', 'id(object) -> int'],
		['hash', 'hash(object) -> int'],
		['callable', 'callable(object) -> bool'],
		['dir', 'dir(object) -> list'],
		['vars', 'vars(object) -> dict'],
		['super', 'super() -> proxy object'],
		['property', 'property(fget, fset, fdel, doc)'],
		['staticmethod', 'staticmethod(function)'],
		['classmethod', 'classmethod(function)'],
		['any', 'any(iterable) -> bool'],
		['all', 'all(iterable) -> bool'],
		['iter', 'iter(object, sentinel)'],
		['next', 'next(iterator, default)'],
		['format', 'format(value, format_spec)'],
		['chr', 'chr(i) -> str'],
		['ord', 'ord(c) -> int'],
		['hex', 'hex(x) -> str'],
		['oct', 'oct(x) -> str'],
		['bin', 'bin(x) -> str'],
		['pow', 'pow(base, exp, mod)'],
		['divmod', 'divmod(a, b) -> (quotient, remainder)'],
		['slice', 'slice(start, stop, step)'],
		['breakpoint', 'breakpoint(*args, **kws)'],
		['Exception', 'Base class for exceptions'],
		['ValueError', 'Inappropriate argument value'],
		['TypeError', 'Inappropriate argument type'],
		['KeyError', 'Key not found in mapping'],
		['IndexError', 'Sequence index out of range'],
		['AttributeError', 'Attribute not found'],
		['FileNotFoundError', 'File not found'],
		['RuntimeError', 'Generic runtime error'],
		['StopIteration', 'Signal end of iteration'],
		['NotImplementedError', 'Method not implemented'],
		['OSError', 'OS-related error'],
		['IOError', 'I/O operation failed']
	];

	const keywords: string[] = [
		'False', 'None', 'True', 'and', 'as', 'assert', 'async', 'await',
		'break', 'class', 'continue', 'def', 'del', 'elif', 'else', 'except',
		'finally', 'for', 'from', 'global', 'if', 'import', 'in', 'is',
		'lambda', 'nonlocal', 'not', 'or', 'pass', 'raise', 'return', 'try',
		'while', 'with', 'yield'
	];

	return [
		...builtins.map(([label, detail]) => ({ label, detail, kind: 'function' as const })),
		...keywords.map((label) => ({ label, kind: 'keyword' as const }))
	];
}

export function getPythonSnippets(): SnippetDef[] {
	return [
		{
			label: 'def',
			insertText: 'def ${1:name}(${2:params}):\n\t${0:pass}',
			detail: 'Function definition'
		},
		{
			label: 'import',
			insertText: 'import ${0:module}',
			detail: 'Import module'
		},
		{
			label: 'from',
			insertText: 'from ${1:module} import ${0:name}',
			detail: 'From import'
		},
		{
			label: 'ifmain',
			insertText: "if __name__ == '__main__':\n\t${0:main()}",
			detail: 'Main guard'
		},
		{
			label: 'main',
			insertText: 'def main(${1:name}):\n\treturn ${0:{}}',
			detail: 'Entry point — params come from inputs'
		},
		{
			label: 'mainctx',
			insertText:
				"def main(${1:name}, ctx):\n\t# ctx: data_interval_start/end, logical_date, ds, ts,\n\t# run_id, is_scheduled, schedule_name, flow_input, steps, ...\n\treturn {'ds': ctx['ds'], ${0:}}",
			detail: 'Entry point with Airflow-style run context'
		},
		{
			label: 'class',
			insertText: 'class ${1:Name}:\n\tdef __init__(self${2:, args}):\n\t\t${0:pass}',
			detail: 'Class definition'
		},
		{
			label: 'try',
			insertText:
				'try:\n\t${1:pass}\nexcept ${2:Exception} as ${3:e}:\n\t${0:raise}',
			detail: 'Try/except block'
		}
	];
}

