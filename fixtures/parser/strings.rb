# encoding: utf-8
# 日本語のコメント
a = "日本語 #{b} です"
c = 'シングル\'クォート\\'
d = "escapes: \t \n \e \u00e9 \u{1F600} \x41 \101 \C-a"
e = %q(percent q #{not})
f = %Q[percent Q #{yes}]
g = %(parens #{also} (nested))
h = ?a
i = ?あ
j = ?\n
k = "adjacent" 'strings' "#{joined}"
l = "line \
continuation"
m = "multi
line
string #{x}
end"
n = 'multi
line single'
o = %w[a b c d\ e]
p1 = %W[a#{b} c d#{e}f]
q = %i[sym bols]
r = %I[a#{b} c]
s = %w[
  first
  second\
  third
]
t = :symbol
u = :"quoted symbol"
v = :"interpolated #{sym}"
w = :あ
x = :'single quoted'
y = %s(percent s)
z = `ls -la`
aa = %x(echo #{x})
bb = :+
cc = :[]=
dd = :foo?
ee = :Foo
ff = :@ivar
gg = :$gvar
hh = :@@cvar
ii = "#@ivar #$gvar #@@cvar"
jj = ""
kk = ''
ll = %w[]
mm = %()
nn = "a" \
  "b"
oo = "\\n not newline \\\\n"
pp1 = "tab\
"
qq = "😀 emoji #{x} 😀"
rr = { "string key": 1, "interp #{x}": 2, 'single': 3 }
ss = "x
#{y}
z"
tt = %W[
  a#{b}
  c
]
uu = __FILE__
vv = __LINE__
ww = __ENCODING__
xx = "#{}"
yy = "#{1}#{2}"
zz = "\#{not interpolated}"
