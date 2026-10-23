def stderr:      (       _stderr_empty  as $x | .), .;
def debug:       (        _debug_empty  as $x | .), .;
def debug(msgs): ((msgs | _debug_empty) as $x | .), .;

def halt: _halt(0);
# Pathfinder: jq 1.8.1 writes a string raw, nothing for null, else JSON and a newline.
def halt_error($exit_code): (if . == null then empty elif . >= "" and . < [] then . else tojson + "\n" end | _stderr_empty), _halt($exit_code);
def halt_error: halt_error(5);

# Not defined in jq!
def _isboolean: . == true or . == false;
def _isnumber:  . > true and . < "";
def _isstring:  . >= ""  and . < [];
def _isarray:   . >= []  and . < {};
def _isobject:  . >= {};

# Numbers
# Pathfinder: `nan` and `infinite` are native: `/` by zero is an error.
def isnan:      . < nan and nan < .;
def isinfinite: . == infinite or  . == -infinite;
def isfinite:   _isnumber and (isinfinite | not);
def isnormal:   _isnumber and ((. == 0 or isnan or isinfinite) | not);

# Math
def abs: if . < 0 then - . end;
def logb:
    if . == 0.0 then -infinite
  elif isinfinite then infinite
  elif isnan then .
  else _ilogb | . + 0.0 end;
def significand:
    if isinfinite or isnan then .
  elif . == 0.0 then 0.0
  else scalbln(.; _ilogb | -1 * .) end;
def _pow10:            pow(10.0; .);
def drem($l; r):      remainder($l; r) | if . == 0 then copysign(.; $l) end;
def nexttoward(x; y): nextafter(x; y);
# Pathfinder: C's `scalb`, NaN for an exponent that is no integer, as in jq.
def scalb(x; e):      e as $e | x as $x | if $e == ($e | trunc) or ($e | isinfinite) then $x * pow(2.0; $e) else nan end;
def gamma: tgamma;

# Type
def type:
    if . == null then "null"
  elif _isboolean then "boolean"
  elif . < "" then "number"
  elif . < [] then "string"
  elif . < {} then "array"
  else             "object" end;

# Selection
def values:    select(. != null);
def nulls:     select(. == null);
def booleans:  select(_isboolean);
def numbers:   select(_isnumber);
def finites:   select(isfinite);
def normals:   select(isnormal);
def strings:   select(_isstring);
def arrays:    select(_isarray);
def objects:   select(_isobject);
def iterables: select(. >= []);
def scalars:   select(. <  []);

# Iterators
def add(f): reduce f as $x (null; . + $x);
def add: add(.[]);

# Arrays
def min_by(f): reduce _min_by_or_empty(f) as $x (null; $x);
def max_by(f): reduce _max_by_or_empty(f) as $x (null; $x);
# Pathfinder: `min` and `max` are native, with jq's message for a non-array.
def unique_by(f): [group_by(f)[] | .[0]];
# Pathfinder: jq's `unique` sorts, and fails as `sort` does.
def unique: if . < [] or . >= {} then sort else unique_by(.) end;

# Paths
def pick(f):
  reduce _path_value(f) as [$path, $value] ({}; . *
    reduce ($path | reverse[]) as $p ($value; {($p): .})
  );

def keys: keys_unsorted | sort;

def flatten: [recurse(arrays[]) | select(_isarray | not)];
def flatten($d): if $d > 0 then map(if _isarray then flatten($d-1) else [.] end) | add end;

# Regular expressions
def _capture_of_match: map(select(.name) | { (.name): .string} ) | add + {};

def    test(re; flags): _matches(re; flags) | any;
def    scan(re; flags): _matches(re; flags)[] | .[0].string;
def   match(re; flags): _matches(re; flags)[] | .[0] + { captures: .[1:] };
def capture(re; flags): _matches(re; flags)[] | _capture_of_match;

def split($sep):
  if _isstring and ($sep | _isstring) then . / $sep
  else error("split input and separator must be strings") end;
def split (re; flags): _split_(re; flags + "g");
def splits(re; flags): split(re; flags)[];

def sub(re; f; flags):
  def handle: if _isarray then _capture_of_match | f end;
  reduce _split_matches(re; flags)[] as $x (""; . + ($x | handle));

def gsub(re; f; flags): sub(re; f; "g" + flags);

def    test(re):    test(re; "");
def    scan(re):    scan(re; "");
def   match(re):   match(re; "");
def capture(re): capture(re; "");
def  splits(re):  splits(re; "");
def  sub(re; f): sub(re; f;  "");
def gsub(re; f): sub(re; f; "g");

# Date
def   todate:   todateiso8601;
def fromdate: fromdateiso8601;

# Formatting
# Pathfinder: `@sh` is native, with jq's message for an array or object.
def @text: "\(.)";
def @html   : tostring | _escape_html;
# Pathfinder: jq 1.8.1 has no `@htmld`.
def @uri    : tostring | _encode_uri;
def @urid   : tostring | _decode_uri;
def @base64 : tostring | _encode_base64;
def @base64d: tostring | _decode_base64;
