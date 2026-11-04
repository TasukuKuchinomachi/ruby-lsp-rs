a = /simple/
b = /options/imx
c = %r{percent r}i
d = /interpolated #{x} here/o
e = /(?<name>\w+) (?<age>\d+)/ =~ line
f = /no captures/ =~ line
g = line =~ /reversed/
h = /#{dynamic}/ =~ line
i = /multi
line
regexp/x
j = /escaped \/ slash/
k = %r{with / slash}
l = /日本語/
m = /\A\s*\z/
n = //
o = /a/ !~ b
if /match current line/
  p 1
end
puts 1 if /modifier/
q = /a/u
r = /a/n
s = /a\
b/
t = %r[
  line one
  line two #{x}
]x
