---
version: "0.3.0"
updated: 2026-11-24
status: draft
---

# FEAN 경제 계약과 구성원 인지의 형식언어

이 언어로 묻는 질문은 **“주어진 시점에 누가 무엇을 알고 어떤 거래를 선택할 수 있으며, 그 선택으로 어느 약속까지 지킬 수 있는가?”**이다. 모델은 선언·정의·프로그램·질의로 이루어진 한 편의 글이다. 이 글은 실행 전에 타입 검사를 받고, 유한 모형에서는 모든 실행 이력을 전수 검사한다. 거꾸로 **“그 약속이 지켜지려면 무엇이 어떤 값이어야 하는가?”**도 같은 글로 묻는다([역산](#solve)). 이 명세의 버전은 0.3이다. 0.2의 모델은 그대로 0.3의 모델이다. [v0.1 명세](financial-language-v0.1.md)는 그대로 보존했다.

[언어의 구조](#language) · [타입과 정적 규칙](#types) · [원자적 실행](#execution) · [이력과 인지](#epistemics) · [분석 질의](#queries) · [역산](#solve) · [표준 계약](#library) · [80원 상환 예제](#example)

이 문서의 ```` ```fean ```` 블록은 완결된 모델이고 ```` ```fean-prelude ```` 블록은 모든 모델 앞에 놓이는 표준 선언이다. [참조 구현](../tests/fean_ref/__init__.py)이 이 블록들을 그대로 읽어 검사하고 실행한다. `query`와 `solve` 선언 뒤의 `// =>` 주석은 그 실행이 내야 하는 결과이며 [검산](../tests/test_fean_v02.py)이 글자까지 대조한다.

## 0. v0.1에서 달라진 점

| v0.1의 문제 | v0.2의 처리 | 절 |
|---|---|---|
| 관찰이 `tell`뿐이라 자기 잔고도 알 수 없었다 | 셀에 관찰자를 선언하고, 자기 보유량은 기본으로 본다 | 4.2 |
| 한 사건 안에서는 순액결제가 기본값이었다 | 총액이 기본이고 순액은 `net`으로 선언한다 | 3.2 |
| 잔고 부족이 거절인지 실패인지가 작성 방식에 달려 있었다 | 재원 요건을 전상태 단계의 판정으로 고정한다 | 3.2 |
| 기억 슬롯이 없는 주체는 같은 자리에서 다른 행동을 못 해 경로가 줄었다 | 한 경로의 메뉴 중복을 정적 오류로 막는다 | 2.4, 4.3 |
| 질의가 말단만 읽었고 확률과 평가가 모형 밖에 있었다 | `at(지점, 식)`, 선언하는 `weight`·`value`, 겹쳐 쓰는 수치 질의 | 5 |
| 타입 규칙이 산문이었고 문법이 잘못된 식을 그대로 생성했다 | 타입·층·구조 규칙에 이름을 붙이고 실행 전에 검사한다 | 2 |
| 이력과 관찰 지점이 정의되지 않았다 | 이력은 걸음의 열, 관찰 지점은 그 접두부다 | 4.1 |
| 현재 시각, 곱셈·나눗셈, 매개변수가 없었다 | `now`, `*`, `floor`·`ceil`·`exact`, 사건·행동·절차의 매개변수 | 1.1, 2 |
| 계약 패턴의 전개가 산문이었다 | `contract` 틀과 `instance`, 언어로 쓴 표준 계약 | 2.5, 6 |

v0.3은 v0.2에 역산을 더한다. 값을 정하지 않은 상수 `unknown`과, 목표가 성립하는 값을 찾는 `solve`다(5.4). 그 밖의 문법과 의미는 v0.2와 같다.

<a id="language"></a>
## 1. 언어의 구조

모델은 `model 이름 { 선언… }`이다. 선언은 네 구역으로 나뉜다.

| 구역 | 선언 | 정하는 것 |
|---|---|---|
| 세계 | `actor`, `asset`, `enum`, `time`, `param`, `unknown`, `cell`, `slot`, `assume`, `invariant` | 구성원, 자산·측정 단위, 공개 시각, 공개 매개변수와 미지수, 상태 셀과 기억 슬롯, 가능한 초기 세계, 불변식 |
| 정의 | `def`, `event`, `batch`, `proc`, `contract`, `instance` | 사건, 제도가 허용한 공동 결제 묶음, 다시 쓰는 함수·절차·계약 틀 |
| 실행 | `program` | 사건을 시각·순서·선택으로 엮은 프로그램 |
| 분석 | `weight`, `value`, `query`, `solve` | 확률 가중치, 이력의 평가값, 질의, 역산 질의 |

모델은 다섯 단계를 거친다. ① 구문 분석 ② 틀과 매개변수의 전개 ③ 타입·구조 검사 ④ 실행, 곧 모든 이력의 열거 ⑤ 질의 평가. ①~③에서 걸리는 모델은 실행되지 않는다. 역산 질의는 후보마다 ④와 ⑤를 되풀이한 결과로 정의한다. 외부 충격의 가능한 값은 초기 세계나 프로그램의 `alt` 가지에 명시한다.

### 1.1 표면 문법

글자 규칙은 다음과 같다. 주석은 `//`부터 줄 끝까지다. 이름은 영문자·숫자·밑줄로 쓰고 예약어는 이름이 될 수 없다. 정수는 10진수다. 수량 리터럴은 `100[KRW]`처럼 단위를 대괄호에 적고, 시각 리터럴은 `@4`처럼 `@`를 붙인다. 줄바꿈과 들여쓰기는 뜻이 없다.

```ebnf
model    = "model" Name "{" { decl } "}" ;
decl     = "actor" Name { "," Name }
         | "asset" Name "in" bound ".." bound [ "init" hinit { "," hinit } ] [ "observe" "all" ]
         | "enum" Name "{" Name { "," Name } "}"
         | "time" Int ".." Int
         | "param" Name ":" type [ "in" bound ".." bound ]
         | "unknown" Name ":" type [ "in" bound ".." bound ]
         | "slot" Name ":" type "for" Name { "," Name }
         | "assume" expr
         | "invariant" expr
         | "def" Name "(" [ params ] ")" ":" type "=" expr
         | "contract" Name "(" [ params ] ")" "{" { member } "}"
         | "program" block
         | "weight" Name "=" expr
         | "value" Name "=" expr
         | "query" Name "=" query
         | "solve" Name "=" solve
         | member ;
member   = "cell" Name ":" type [ "in" bound ".." bound ] "init" init
               [ "write" QName { "," QName } ] [ "observe" ( "all" | Name { "," Name } ) ]
         | "event" Name [ "(" params ")" ] "by" expr { "," expr } [ "net" ] "{" { clause } "}"
         | "batch" Name "=" [ "net" ] "{" call { "," call } "}"
         | "proc" Name "(" [ params ] ")" block
         | "instance" Name "=" Name "(" [ args ] ")" ;
hinit    = Name ( "=" expr | "in" "{" args "}" ) ;
init     = expr | "in" "{" args "}" ;
bound    = sum ;
params   = Name ":" type { "," Name ":" type } ;
args     = expr { "," expr } ;
type     = base [ "?" ] ;
base     = "Bool" | "Time" | "Actor" | "Asset" | "Num" | "Qty" "[" unit "]" | Name ;
unit     = Name { ( "*" | "/" ) Name } ;

clause   = "auth" expr | "require" expr | "given" Name "=" expr | "let" Name "=" expr
         | "ensure" expr
         | target ":=" expr | target ( "+=" | "-=" ) expr | "free" target
         | "move" expr "->" expr ":" expr
         | "tell" Name "." Name ":=" expr "asof" expr
         | "forget" Name "." Name ;
target   = QName | "bal" "(" Name "," Name ")" ;

block    = "{" { stmt } "}" ;
stmt     = "run" unitref
         | "attempt" unitref block [ "else" block ]
         | "at" expr block
         | "alt" block "or" block { "or" block }
         | "choose" Name "menu" Name "{" action { action } "}"
         | "do" QName "(" [ args ] ")"
         | "label" Name ;
unitref  = "batch" QName | call ;
call     = QName [ "(" [ args ] ")" ] ;
action   = Name [ "(" Name "in" ( "{" args "}" | bound ".." bound ) ")" ] "=>" block ;

expr     = disj ;
disj     = conj { "or" conj } ;
conj     = neg { "and" neg } ;
neg      = "not" neg | cmp ;
cmp      = sum [ ( "==" | "!=" | "<" | "<=" | ">" | ">=" ) sum ] ;
sum      = prod { ( "+" | "-" ) prod } ;
prod     = unary { "*" unary } ;
unary    = "-" unary | dflt ;
dflt     = atom { "??" atom } ;
atom     = Int [ "[" unit "]" ] | "@" Int | "true" | "false" | "empty" | "now"
         | "if" expr "then" expr "else" expr
         | ( "floor" | "ceil" | "exact" ) "(" sum "/" sum ")"
         | ( "min" | "max" ) "(" expr "," expr ")"
         | "known" "(" expr ")"
         | ( "mem" | "asof" | "recv" ) "(" Name "," Name ")"
         | "bal" "(" expr "," Name ")"
         | "post" "(" target ")"
         | "K" "(" Name "," expr ")"
         | "at" "(" point "," expr ")" | "reach" "(" point ")"
         | QName [ "(" [ args ] ")" ]
         | "(" expr ")" ;
point    = "start" | "end" | "menu" "(" QName ")" | QName ;

query    = "exists" "run" ":" expr
         | "forall" "run" ":" expr
         | "can" coalition ":" expr
         | [ "max" "policy" coalition ":" ] [ "expect" Name ":" ] ( "max" | "min" ) "run" ":" expr
         | "region" Name { "," Name } ":" query
         | "prefer" Name "at" point ":" Name "over" Name "by" Name [ "under" Name ]
         | ( "POSSIBLE" | "ALL" ) "(" expr ")"
         | "CAN" "(" coalition "," expr ")" ;
solve    = ( "find" | "any" | ( "min" | "max" ) expr ) [ "at" "(" QName ")" ] [ "where" expr ] ":" expr ;
coalition = "{" [ Name { "," Name } ] "}" ;
QName    = Name { "." Name } ;
```

식의 문법은 하나다. 어느 자리에 어느 구성자를 쓸 수 있는지는 2.2의 층이 가른다. 문법에 맞지 않는 글은 줄 번호와 함께 거절된다. 모르는 글자는 `S-Lex`, 기대와 다른 토큰은 `S-Parse`다.

### 1.2 핵심 구문과 풀어쓰기

표면 문법은 아래의 핵심 구문으로 풀린다. 타입 규칙과 실행 의미는 핵심 구문에 대해 정의한다. 핵심 구문의 이름은 v0.1과 같다.

| 표면 | 핵심 구문 |
|---|---|
| `c := e` | `set(c, e)` |
| `c += e`, `c -= e` | `delta(c, e)`, `delta(c, minus(0, e))` |
| `free c` | `free(c)` |
| `move a -> b : e` | `move(a, b, x, e)`. x는 e의 단위인 자산이다 |
| `tell i.k := v asof u`, `forget i.k` | `tell(i, k, v, u)`, `forget(i, k)` |
| `require φ`, `auth φ` | 사전식과 권한식의 논리곱에 φ를 더한다 |
| `given x = e` | `require known(e)`와 x의 결속 |
| `{ s₁ … sₙ }`, `{ }` | `seq(s₁, seq(…, sₙ))`, `done` |
| `run U`, `attempt U {P} else {Q}` | `run(U)`, `attempt(U, P, Q)`. U는 사건 하나 또는 묶음이다 |
| `label ℓ` | `mark(ℓ)` |
| `do p(e…)` | p의 몸을 그 자리에 전개한다 |
| `a != b`, `a < b`, `a > b`, `a >= b` | `not(eq(a,b))`, `and(le(a,b), not(eq(a,b)))`, `not(le(a,b))`, `le(b,a)` |
| `POSSIBLE(G)`, `ALL(G)`, `CAN(C, G)` | `exists run: at(end, G)`, `forall run: at(end, G)`, `can C: at(end, G)` |
| v0.1의 `REGION(θ, Q)`, `PREF(i, a, b)` | `region θ: Q`, `prefer i at menu(m): a over b by v` |

<a id="types"></a>
## 2. 타입과 정적 규칙

### 2.1 타입

```math
\tau ::= \mathtt{Bool}\mid\mathtt{Time}\mid\mathtt{Actor}\mid E\mid\mathtt{Qty}[u]\mid\tau?
```

- `Qty[u]`는 단위 u의 정수 수량이다. 단위는 선언된 자산의 곱과 몫이며 `Qty[KRW]`와 `Qty[share]`는 다른 타입이다. `Num`은 단위 없는 정수다. 식의 값은 부호를 가질 수 있고, 범위는 셀의 제약이다. 상·하한을 넘긴 값의 자동 반올림이나 포화 처리는 없다.
- `E`는 `enum`으로 선언한 열거형, `Time`은 `time lo..hi`로 선언한 공개 시각, `Actor`는 선언된 구성원이다.
- `τ?`는 τ의 값 또는 빈 값 `empty`다. τ의 값은 `τ?`가 필요한 자리에 쓸 수 있다. 빈 값을 푸는 방법은 `known(e)`, `e ?? d`, 사건의 `given x = e` 세 가지뿐이다.
- `Asset`은 틀과 절차의 매개변수에만 쓰며 값으로 계산하지 않는다.
- 선언부의 범위와 초기값에서 단위 없는 정수는 그 셀의 단위로 읽는다. 식에서는 단위를 반드시 적는다.

### 2.2 식의 층

| 층 | 쓰는 자리 | 읽을 수 있는 것 |
|---|---|---|
| C 상수 | 인수, 초기값, 범위, `at`의 시각, 지점 없는 `solve`의 `where` | 리터럴, `param`, `unknown`, 열거값, 구성원, 상수인 `def` |
| S 상태 | `require`·`auth`·`let`·`given`과 효과의 우변, `by`, `def`의 몸, `invariant`, `assume`, `weight`, `solve … at(ℓ)`의 `where` | C에 더해 셀, `bal`, `mem`·`asof`·`recv`, `now` |
| P 사후 | `ensure` | S에 더해 `post(c)` |
| Q 지점 | `at(ℓ, ·)`의 안쪽 | S에 더해 `K(i, φ)` |
| R 이력 | `value`, 질의의 몸 | C에 더해 `at(ℓ, e)`, `reach(ℓ)`, 선언된 `value` |
| V 목표 | 지점 없는 `solve`의 목표와 `min`·`max`의 식 | C에 더해 질의의 이름 |
| X 지점 목표 | `solve … at(ℓ)`의 목표와 `min`·`max`의 식 | S에 더해 질의의 이름. 셀과 `now`는 그 지점의 상태에서 읽는다 |

지식 술어 `K`는 Q층에만 있으므로 사건의 조건이나 효과에 쓸 수 없다. 전이가 지식에 의존하지 않으므로 이력의 집합을 먼저 만들고 그 위에서 지식을 계산할 수 있다.

### 2.3 타입 규칙

판단은 “환경 Γ와 층 m에서 식 e의 타입이 τ다”이다. 규칙 이름은 참조 구현이 내는 오류 이름과 같다.

| 규칙 | 조건 | 타입 |
|---|---|---|
| T-Lit | `n`; `n[u]`에서 u의 자산이 선언됨; `@n`에서 n이 선언된 시각; `true`·`false`; `empty` | Num; Qty[u]; Time; Bool; 어느 τ?에도 맞는 빈 값 |
| T-Name | 이름이 지역 결속, 매개변수, 미지수, 셀, 열거값, 구성원 가운데 하나로 풀린다. V·X층에서는 질의의 이름을 이들보다 먼저 찾는다. `def`의 몸은 목표가 불러도 V·X층이 아니므로 그 안의 이름은 질의가 아니다 | 선언된 타입. 질의의 이름은 Q-Solve의 타입 |
| T-Layer | 셀·`bal`·`mem`·`asof`·`recv`·`now`는 S·P·Q·X층, `post`는 P층, `K`는 Q층, `at`·`reach`·평가값 이름은 R층 | 그 밖의 층에서는 오류 |
| T-Arith | `a + b`, `a - b`, `min(a,b)`, `max(a,b)`: 두 쪽이 옵션 아닌 같은 Qty[u]. `min`·`max`는 Time도 된다. `-a`: Qty[u] | Qty[u] |
| T-Arith | `a * b`: Qty[u]와 Qty[v] | Qty[u·v] |
| T-Arith | `floor(a / b)`, `ceil(a / b)`, `exact(a / b)`: Qty[u]와 Qty[v] | b가 0 아닌 리터럴인 `floor`·`ceil`은 Qty[u/v], 그 밖에는 Qty[u/v]? |
| T-Cmp | `<`, `<=`, `>`, `>=`: 두 쪽이 옵션 아닌 같은 Qty[u] 또는 Time | Bool |
| T-Eq | `==`, `!=`: 옵션을 벗긴 타입이 같다 | Bool |
| T-Bool | `and`, `or`, `not`의 인수, `K(i, φ)`의 φ, `require`·`auth`·`ensure`·`invariant`·`assume`의 식이 Bool이다 | Bool |
| T-If | `if c then a else b`: c는 Bool, a와 b는 옵션을 벗기면 같은 타입 | 한쪽이라도 옵션이면 τ?, 아니면 τ |
| T-Opt | `known(e)`; `e ?? d`에서 e와 d는 옵션을 벗기면 같은 타입 | Bool; d의 타입 |
| T-Mem | `mem(i,k)`, `asof(i,k)`, `recv(i,k)`: 슬롯 k가 i에게 선언됨 | τ?, Time?, Time? |
| T-Bal | `bal(a, x)`: a는 Actor, x는 선언된 자산 | Qty[x] |
| T-Call | `f(e…)`: 인수 타입이 매개변수 타입에 맞는다. C·R·V층에서는 f의 몸도 상수다 | f의 결과 타입 |
| T-Given | `given x = e`: e가 τ? | x는 τ |

`post(c)`의 타입은 c의 타입이다. `at(ℓ, e)`는 e가 Bool이면 Bool이고 그 밖에는 τ?다. `reach(ℓ)`은 Bool이다.

| 규칙 | 효과와 조건 |
|---|---|
| E-Set | `c := e`: c는 보유량이 아닌 셀이고 e의 타입이 c의 타입에 맞는다 |
| E-Delta | `c += e`, `c -= e`: c는 옵션 아닌 Qty[u] 셀 또는 `bal(i, x)`이고 e는 같은 단위다 |
| E-Free | `free c`: c는 보유량이 아닌 셀이다 |
| E-Move | `move a -> b : e`: a와 b는 Actor, e는 자산 하나의 Qty[x]다 |
| E-Tell | `tell i.k := v asof u`: 슬롯 k가 i에게 선언됨, v는 슬롯 타입, u는 Time. `forget i.k`도 선언된 슬롯이다 |

| 규칙 | 프로그램·질의와 조건 |
|---|---|
| P-Unit | `run`·`attempt`·묶음의 구성원·`do`가 선언된 사건·묶음·절차를 가리킨다 |
| P-Args | 인수의 개수와 타입이 매개변수와 맞고 C층의 식이다 |
| P-Choose | 한 메뉴의 행동 이름이 서로 다르고 행동 매개변수의 값은 리터럴이다 |
| Q-Bool | `exists`·`forall`·`can`의 식과 `region`의 안쪽은 불이다 |
| Q-Num | `max·min run`의 식은 옵션 아닌 Qty[u], `expect`의 이름은 선언된 `weight`이고 `weight`는 S층의 Num이다 |
| Q-Point | 지점은 `start`, `end`, 프로그램에 있는 `label`, 프로그램에 있는 `menu(m)`이다 |
| Q-Region | 이름은 선언된 `param`이고 안쪽은 불 질의다 |
| Q-Prefer | 지점은 `menu(m)`, 두 행동은 그 메뉴의 행동, 평가값은 선언된 수치 `value`다 |
| Q-Solve | `solve`의 목표와 `where`는 불이고 `min`·`max`의 식은 옵션 아닌 Qty[u]다. 목표에 쓰는 질의의 이름은 불 질의이면 Bool, `expect` 없는 수치 질의이면 그 Qty[u]이며, 영역 질의와 `expect` 질의의 이름은 쓸 수 없다. `at`이 없으면 미지수가 하나 이상 있어야 하고, `at(ℓ)`의 ℓ은 프로그램에 한 번만 적힌 `label`이다 |

**평가의 전체성.** 위 규칙을 통과한 식은 그 층의 어느 상태에서도 자기 타입의 값으로 평가된다. 구성자마다 전체 함수이고 `def`는 재귀하지 않기 때문이다. 0으로 나누거나 `exact`가 나누어떨어지지 않으면 빈 값이 되고, 그 타입은 옵션이라 반드시 풀어야 한다. 따라서 타입·단위·정의역의 잘못은 실행 전에, 도달 여부와 무관하게 모두 드러난다.

### 2.4 구조 규칙

| 규칙 | 조건 |
|---|---|
| W-Name | 같은 종류의 이름은 한 번만 선언한다. 식에 쓰는 이름(셀, 매개변수, 미지수, 열거값, 구성원, 평가값, 인스턴스)은 서로 겹치지 않는다. `let`·`given`·매개변수의 이름은 이들을 가리지 않는다. `query`와 `solve`의 이름도 서로 겹치지 않는다 |
| W-Range | Qty·Num 타입의 셀과 매개변수와 미지수는 범위를 선언한다. 하한은 상한보다 크지 않고 초기값은 범위 안이다 |
| W-Writer | 최상위 셀은 `write`에 적힌 사건만 쓴다. 틀의 셀은 같은 인스턴스의 사건만 쓴다 |
| W-Write | 한 커밋(사건 하나 또는 묶음 전체)에서 `:=`와 `free`는 한 셀에 한 번이고 그 셀에는 `+=`·`-=`가 없다. 한 슬롯에는 `tell`·`forget`이 한 번이다 |
| W-Debit | `move a -> b`의 a가 `by`에 적힌 식 가운데 하나와 같거나, 그 사건에 `auth`가 있다. 보유량의 `+=`·`-=`, 곧 수량의 생성과 소멸에는 `auth`가 있어야 한다 |
| W-Batch | `run batch b`와 `attempt batch b`의 b가 선언되어 있고 구성원 사건이 서로 다르다 |
| W-Clock | `at t`의 t는 선언된 시각이고, 앞선 문장들이 도달할 수 있는 가장 늦은 시각보다 이르지 않다 |
| W-Menu | 순서대로 이어지는 문장들에 같은 메뉴 이름이나 같은 지점 이름이 두 번 나오지 않는다. 서로 배타적인 가지에는 같은 이름을 쓸 수 있다 |
| W-Rec | `def`·`proc`·`contract`는 자신을 직접으로도 간접으로도 부르지 않는다 |
| W-Static | `tell`·`forget`·`mem`·`K`·`choose`의 구성원과 쓰기 대상의 셀은 전개 뒤에 이름으로 정해져 있다 |

`by`에 적는 실행 주체는 W-Debit의 기준이자 이력의 사건 기록에 남는 이름이다. `auth φ`는 실행 주체가 아닌 구성원의 보유량을 줄이거나 수량을 생성·소멸하는 근거이며, 실행 때 거짓이면 사건은 거절된다. 범위와 `at`의 시각이 `param`에 달려 있으면 W-Range와 W-Clock은 매개변수 배정마다 실행 직전에 검사한다. 미지수에 달려 있으면 후보마다 검사하고, 어긴 후보는 오류가 아니라 해가 아닌 값이 된다(5.4).

실행 전에 걸러지지 않는 모델 오류는 셋뿐이다. 초기 세계가 하나도 없거나 초기 상태가 범위·불변식을 어기는 경우(`D-Init`), 이전량이 음수로 계산된 경우(`D-Move`), 가중치가 음수이거나 합이 0인 경우(`D-Weight`)다. 앞의 것은 실행 직전에, 뒤의 둘은 그 평가에 도달했을 때 `INVALID_MODEL`로 보고한다.

### 2.5 틀과 매개변수의 전개

- `def f(x : τ…) : τ = e`는 이름 붙인 식이다. 부른 자리의 상태에서 몸을 평가한다.
- `event e(x : τ…)`의 매개변수는 값이다. `run e(v…)`의 인수는 상수 식이고, 인수마다 다른 기본 사건이 된다.
- `proc p(x…) { … }`는 프로그램 조각이다. `do p(v…)`는 인수를 넣은 몸으로 바뀐다.
- `choose`의 행동 `a(x in {v₁, …}) => { … }`는 값마다 하나의 행동 `a(v₁)`, …으로 펴진다. 주문량·한도가격처럼 결정자가 직접 정하는 인수가 행동에 포함되므로, 같은 시야에서 같은 이름을 고르고 숨은 세계에 따라 다른 수량을 보내는 정책은 쓸 수 없다.
- `contract C(x…) { … }`는 셀·사건·묶음·절차·하위 인스턴스를 가진 틀이다. `instance c = C(v…)`는 몸을 복사해 매개변수를 인수로 바꾸고 구성 이름 앞에 `c.`를 붙인다. 틀 안의 메뉴와 지점 이름에도 `c.`가 붙는다. 하위 인스턴스는 `c.d.…`로 펴진다. 같은 담보·주문·청구권을 여러 계약이 참조하면 같은 인스턴스를 가리킨다.

전개가 끝나면 모델은 유한한 기본 선언만 남고, 2.3과 2.4의 규칙은 이 결과에 적용한다. 틀 안의 타입 오류는 인스턴스를 만든 자리에서 드러난다.

```fean
model Order {
  actor T
  time 0..0
  cell cheap  : Bool        init in { true, false }
  cell bought : Num in 0..2 init 0 write buy

  event buy(n : Num) by T { bought := n }

  program {
    choose T menu order { bid(n in 0..2) => { run buy(n) } }
  }

  query lucky  = exists run: at(end, cheap and bought == 2)                                // => true
  query hedged = can {T}: at(end, (cheap and bought == 2) or (not cheap and bought == 0))   // => false
}
```

T는 `cheap`을 관찰하지 못한다. 싼 세계에서 2주를 사는 이력은 있지만, 세계마다 다른 수량을 내는 정책은 없다.

<a id="execution"></a>
## 3. 원자적 전이와 프로그램

### 3.1 상태와 초기 세계

상태는 `s = (t, σ, μ)`다. t는 공개 시각, σ는 셀과 보유량 `bal(i, x)`의 값, μ는 구성원별 기억 슬롯이다. 슬롯은 비어 있거나 (내용, 사실 기준시각, 수신시각)이다.

초기 세계의 집합 W는 선언에서 나온다. `init v`는 모든 세계에서 같은 값이고 `init in { v₁, … }`은 세계마다 다를 수 있는 값이다. W는 이 선택들의 모든 조합 가운데 `assume`을 만족하는 것이다. 초기 시각은 선언된 가장 이른 시각이고 모든 슬롯은 비어 있다. 실제 세계는 W에 포함된다고 가정한다. `param`은 모든 구성원이 아는 공개 상수이며 값마다 별개의 모형이 된다.

### 3.2 한 커밋의 판정

커밋 단위 U는 사건 하나 또는 선언된 묶음이다. 묶음의 사건들은 모두 같은 전상태 s와 같은 후상태 s′를 본다.

```math
\mathrm{Next}_U(s)=\{s'\mid \mathrm{Pre}_U(s)\land\mathrm{Fund}_U(s)\land R_U(s,s')
\land\mathrm{Range}(s')\land\mathrm{Inv}(s')\land\mathrm{Post}_U(s,s')\}.
```

1. **결속과 조건.** 사건마다 절을 적힌 순서로 읽는다. `let`과 `given`은 전상태의 값을 이름에 묶고, `given`의 값이 비어 있거나 `auth`·`require`가 거짓이면 멈춘다. 이것이 Pre다.
2. **흐름 계산.** 효과의 우변은 모두 전상태에서 평가한다. `:=`는 대입, `+=`·`-=`는 부호 있는 증감, `move a -> b : n`은 두 보유량에 −n과 +n, `free`는 선언된 범위의 모든 값, `tell`·`forget`은 지정한 슬롯이다. 이 목록이 R이다. 쓰지 않은 셀과 슬롯은 전상태 값을 보존한다.
3. **재원.** 보유량 h마다 나가는 양 out(h)는 그 보유량에서 나가는 이전과 음의 증감의 합, 들어오는 양 in(h)는 들어오는 이전과 양의 증감의 합이다. 하한을 lo라 하면 Fund는 다음과 같다.
   - 총액(기본): 모든 h에서 `s(h) − out(h) ≥ lo`
   - 순액(`net`을 선언한 사건·묶음): 모든 h에서 `s(h) + in(h) − out(h) ≥ lo`. 묶음 안에서는 묶음의 선언만 기준이 된다
4. **후보.** 미정 셀을 범위에서 열거해 후상태 후보를 만든다.
5. **검증과 커밋.** 쓴 셀의 범위, 불변식, 모든 사건의 `ensure`를 만족하는 후보만 남긴다. 남은 후보마다 경로가 하나씩 생긴다. 실패하면 장부와 기억 가운데 어느 것도 일부만 바뀌지 않는다.

| 판정 | 조건 | `run`에서 | `attempt`에서 |
|---|---|---|---|
| `BLOCKED` | 1 또는 3이 거짓. 사유는 `given`, `auth`, `require`, `funds` | 실패 말단 | 상태를 보존하고 `else` 블록을 실행 |
| `NO_SETTLEMENT` | 1과 3은 참인데 5의 후보가 0개 | 실패 말단 | 실패 말단 |
| 커밋 | 후보가 1개 이상 | 후보마다 한 경로 | 후보마다 한 경로, 이어서 본문 블록 |

재원 요건은 사전식과 같은 단계에서 판정한다. 그래서 같은 잔고 부족은 사전식에 적었든 적지 않았든 `BLOCKED`다. `NO_SETTLEMENT`는 조건은 통과했으나 범위·불변식·사후식을 함께 만족하는 후상태가 없다는 뜻이다. 복수 후보 가운데 고를 결정권자가 있다면 그 선택을 해당 주체의 `choose`로 적는다.

두 주체가 서로 100원씩 보내야 하고 보유 현금이 0원인 경우를 보자. 총액으로는 어느 쪽도 보낼 재원이 없다. 제도가 순액결제를 허용한 묶음에서만 성립한다. 묶는 것만으로 총액 요건이 사라지지는 않는다. 서로의 청구권을 한꺼번에 소멸시키는 상계는 이와 별개의 사건이다. [금융시장인프라 원칙의 연계 결제 규칙](https://www.bis.org/publications/principles-financial-market-infrastructures.pdf)

```fean
model MutualDebt {
  actor Ann, Bob
  asset KRW in 0..100
  time 0..0

  event payAB by Ann { move Ann -> Bob : 100[KRW] }
  event payBA by Bob { move Bob -> Ann : 100[KRW] }
  batch gross  = { payAB, payBA }
  batch netted = net { payAB, payBA }

  program {
    attempt batch gross  { label grossDone } else { label grossRefused }
    attempt batch netted { label netDone }   else { label netRefused }
  }

  query grossBlocked = forall run: reach(grossRefused)                                        // => true
  query netSettles   = forall run: reach(netDone)                                             // => true
  query unchanged    = forall run: at(end, bal(Ann, KRW) == 0[KRW] and bal(Bob, KRW) == 0[KRW]) // => true
}
```

### 3.3 프로그램의 걸음

프로그램은 걸음을 하나씩 낸다. 반복이 없으므로 모든 실행은 끝난다.

| 문장 | 걸음 | 뜻 |
|---|---|---|
| `at t { P }` | `tick(t)` | 공개 시계를 t로 옮긴 뒤 P를 실행한다. 같은 시각 안의 순서는 프로그램이 정한다. 시각의 경과가 비공개 사건을 알려 주지는 않는다 |
| `run U` | `commit(U)` 또는 `fail(U, 판정)` | 필수 실행이다. 실패하면 원인과 전상태를 가진 실패 말단이 남고 뒤 문장은 이어지지 않는다 |
| `attempt U { P } else { Q }` | `commit(U)` 뒤 P, 또는 `reject(U, 사유)` 뒤 Q, 또는 `fail` | 거절이 계약상 정상 동작일 때 쓴다. 거절을 누가 알게 되는지는 Q에서 명시한다 |
| `alt { P } or { Q }` | `branch(k)` | 외부 사건이나 미정 순서처럼 환경이 고르는 가지다. 이미 정해진 순서는 한 경로로 적는다 |
| `choose i menu m { a => P … }` | `show(i, m, 행동들)` 뒤 `pick(i, m, a)` | i에게 메뉴가 표시되고 i가 자신의 시야로 한 행동을 고른다 |
| `label ℓ` | `mark(ℓ)` | 상태를 바꾸지 않는 지점 이름이다 |
| 블록의 끝 | `end` | 정상 종료다 |

상태를 읽어 가지를 나누려면 조건만 가진 사건을 `attempt`한다. 독립 사건의 허용 순서가 여럿이면 `alt`로 그 순서들을 적는다. 시뮬레이터의 커밋 가능성과 법적·운영상 가능한 결제가 일치하도록 경제적 원자 단위를 정한다.

### 3.4 순서와 사후조건의 검산

- **주문 취소.** 펀드가 2주를 즉시 팔아 180원을 갚는다. 남은 매수 주문은 100원 2주와 70원 1주, 수수료와 다른 재원은 0이다. 100원 주문 1주의 취소가 체결 전이면 170원, 체결 후이면 200원이다. 뒤의 취소 시도는 이미 체결된 수량을 취소할 수 없어 거절된다. 두 순서가 모두 허용되면 상환 가능한 순서와 부족한 순서가 각각 있다.
- **경제적 미이행.** 60원으로 만기 100원 가운데 60원을 지급하고 40원이 남는 것은 정상 후상태다. 이 사례에서 전액상환 목표는 거짓이다. 정상적인 부분상환을 기술적 롤백으로 바꾸면 잔존채권이 틀려진다.
- **최종 미지급을 읽는 지원 약정.** 가용 현금 0원, 채무 100원이다. 지원액과 지원 후 미지급 여부를 서로의 후상태로 정하면 해가 없다. 지원 조건을 전상태의 자금 부족으로 읽으면 해가 하나다.

```fean
model Support {
  actor G, F
  asset KRW in 0..100 init G = 100
  time 0..0
  cell aid    : Qty[KRW] in 0..100 init 0     write aidAfter, aidBefore
  cell unpaid : Bool               init false write aidAfter, aidBefore

  event aidAfter by G {
    free aid  free unpaid
    ensure post(aid) == (if post(unpaid) then 100[KRW] else 0[KRW])
    ensure post(unpaid) == (post(aid) < 100[KRW])
  }
  event aidBefore by G {
    let short = bal(F, KRW) < 100[KRW]
    free aid  free unpaid
    ensure post(aid) == (if short then 100[KRW] else 0[KRW])
    ensure post(unpaid) == (post(aid) < 100[KRW])
  }

  program {
    alt { attempt aidAfter { label afterOk } } or { run aidBefore  label beforeOk }
  }

  query circular = exists run: reach(afterOk)                                 // => false
  query prior    = exists run: at(beforeOk, aid == 100[KRW] and not unpaid)   // => true
  query allEnd   = forall run: reach(end)                                     // => false
}
```

`aidAfter`는 `attempt` 안에 있어도 `NO_SETTLEMENT` 실패 말단으로 끝난다. 그래서 `allEnd`가 거짓이다.

<a id="epistemics"></a>
## 4. 이력, 관찰 지점, 인지

### 4.1 이력과 관찰 지점

**이력**은 하나의 초기 세계에서 시작해 3.3의 걸음이 이어진 유한한 열이다. 걸음마다 그 직후의 상태가 정해진다. **완결 이력**은 `end` 또는 `fail`로 끝난 이력이다. **관찰 지점**은 완결 이력의 접두부다. 빈 접두부, 곧 초기 세계만 정해진 지점도 포함한다. `H_M`은 모든 완결 이력의 모든 접두부의 집합이다.

어떤 걸음의 **직전**은 그 걸음을 뺀 접두부, **직후**는 그 걸음까지의 접두부다. 공개 시각이 같아도 걸음 전후의 상태와 기억은 다를 수 있다. 과거 지점을 질의할 때에는 그 뒤에 들어온 자료를 쓰지 않는다.

| 지점 표기 | 가리키는 접두부 |
|---|---|
| `start` | 빈 접두부 |
| `menu(m)` | `show(i, m, …)`으로 끝나는 접두부. 메뉴가 표시되고 아직 고르기 전, 곧 선택 직전이다 |
| `ℓ` | `mark(ℓ)`으로 끝나는 접두부 |
| `end` | `end`로 끝난 완결 이력. 실패 말단에는 `end`가 없다 |

W-Menu 덕분에 한 이력은 같은 이름의 지점을 많아야 한 번 지난다.

### 4.2 관찰과 시야

구성원이 정보를 얻는 길은 둘이다.

**수신.** `tell i.k := v asof u`가 공개 시각 t에 커밋되면 i의 슬롯은 `(v, u, t)`가 된다. v는 받은 내용, u는 그 내용의 기준시각, t는 수신시각이다. `mem(i,k)`, `asof(i,k)`, `recv(i,k)`가 세 필드를 읽고, 비어 있으면 `empty`다. 대상이 아닌 사람의 슬롯은 그대로다. 같은 슬롯의 정정은 그 시점 이후의 값을 바꾼다. `forget i.k`는 현재 슬롯을 비우되 과거 이력은 그대로 둔다. 두 보고를 모두 기억하려면 서로 다른 슬롯을 쓴다. 자신의 선택을 이후에도 기억하는 주체는 그 선택을 자기 슬롯에 남긴다.

**관찰.** 셀의 `observe`에 적힌 구성원은 그 셀의 현재 값을 언제나 본다. `observe all`은 모두에게 공개한다. 자기 보유량 `bal(i, x)`는 선언하지 않아도 본인이 본다. `asset x … observe all`이면 그 자산의 모든 보유량이 공개된다. 공용 원장의 모든 행, 비공개 분기의 이름, 걸음의 번호는 누구의 시야에도 자동으로 들어가지 않는다.

```math
\mathrm{view}_i(p)=\bigl(t,\ d_i(p),\ \mu_i,\ \sigma|_{\mathrm{Obs}(i)}\bigr),\qquad
p\sim_i q\iff\mathrm{view}_i(p)=\mathrm{view}_i(q).
```

t는 공개 시각이다. `dᵢ(p)`는 p가 `show(i, m, 행동들)`으로 끝나면 그 메뉴 이름과 행동 목록이고 아니면 빈 값이다. 그래서 메뉴 자체가 주는 정보도 계산된다. μᵢ는 i의 슬롯들, 마지막은 i가 관찰하는 셀들의 현재 값이다. 규칙·타입·가능 세계의 범위·`param`은 공통으로 주어진다. 규칙을 일부만 아는 사례는 적용 규칙의 판을 셀에 두고 그 공지를 받은 사람에게만 전달한다.

같은 공개 시각 안에서 수신도 관찰도 없는 구성원은 걸음의 전후를 구별하지 못한다. 예를 들어 0원을 받은 구성원은 같은 시각 안에서는 “지급이 0원이었다”와 “지급이 아직 실행되지 않았다”를 가르지 못하고, 공개 시계가 다음 시각으로 넘어가야 앞의 것임을 안다. 관찰 선언을 빠뜨리면 그 구성원은 실제보다 적게 아는 것으로 계산된다. 이는 그 구성원의 보장 능력에는 보수적이지만 “모른다”는 판정에는 과대하다.

### 4.3 전략과 지식

주체 i의 결정론적 전략은 `πᵢ(viewᵢ) → 행동`이다. 같은 시야에서는 같은 행동을 고른다. 세계의 실제 상태는 사건의 성공 여부를 바꿀 수 있지만, 선택자가 모르는 사실은 전략의 입력이 될 수 없다. 표시된 행동이 숨은 조건 때문에 거절될 수 있는 사례는 실제로 시도한 뒤의 거절 경로로 적는다. 숨은 사실에 따라 메뉴를 미리 좁히면 메뉴가 정보를 흘린다.

**전략 실현.** W-Menu를 지킨 모델에서는 한 이력에서 같은 주체가 같은 시야로 두 번 선택하는 일이 없다. 선택 지점마다 메뉴 이름이 다르기 때문이다. 따라서 모든 완결 이력은 어떤 시야 기반 결정론적 전략들로 실현되고, `H_M`은 정보 제약을 지키는 모든 전략과 허용된 모든 가지에서 나오는 접두부의 집합과 같다. 특정 정책이나 분석 목표에 유리한 미래만 남기기 전의 집합이다.

```math
I_i(p)=\{q\in H_M\mid p\sim_i q\},\qquad
M,p\models K(i,\varphi)\iff\forall q\in I_i(p),\ M,q\models\varphi.
```

p 자신도 `Iᵢ(p)`에 들어가므로 아는 것은 참이다. 같은 실제 세계라도 수신 순서가 다르면 다른 지점이다.

| 식 | 의미 |
|---|---|
| `regime == boom` | 세계의 실제 상태가 호황이다 |
| `mem(D, report) == boom` | D의 기록에 호황이라는 보고가 남아 있다. 같은 보고가 불황에도 올 수 있다 |
| `K(D, regime == boom)` | D의 시야와 양립하는 모든 지점이 호황이다 |
| `not K(D, φ) and not K(D, not φ)` | D의 정보로는 참과 거짓이 모두 가능하다 |
| `proof == boom` | 집행기관이 그 절차의 증거로 호황을 수리했다. `proof`는 권한 있는 사건만 쓰는 셀이다 |

`K`는 **주어진 정보로 논리적으로 확정 가능한 범위**다. 실제로 그 사람이 생각한 결론, 신뢰도, 오판은 별도의 슬롯이나 명시한 평가값에 기록한다. 부정확할 수 있는 보고를 받았다는 사실만으로 그 내용이 참이 되지 않는다. 제3자 집행이 증거를 요구하는 계약은 집행기관의 증거 셀을 조건으로 읽는다. 사실의 진위, 담당자가 아는 내용, 증거로 인정된 내용이 각각 달라지는 사례도 이렇게 적는다. [Halpern–Moses의 개인 상태와 지식 의미론](https://www.cs.cornell.edu/home/halpern/papers/common_knowledge.pdf) · [Hart–Holmström의 관찰·검증 구분](https://economics.mit.edu/sites/default/files/2022-09/Theory%20of%20Contracts-1987.pdf)

<a id="queries"></a>
## 5. 분석 선언과 질의

### 5.1 지점, 가중치, 평가값

질의가 쓰는 의미는 모델 안에 선언한다.

- **지점.** 프로그램에 `label ℓ`을 두거나 `menu(m)`, `start`, `end`를 쓴다. 이력 식 `at(ℓ, e)`는 이력이 ℓ을 지날 때 그 지점에서 e를 평가한 값이다. e가 불이면 “ℓ을 지났고 그 지점에서 참”이고, 불이 아니면 ℓ을 지나지 않은 이력에서 빈 값이다. `reach(ℓ)`은 지났는지 여부다.
- **가중치.** `weight w = e`는 초기 세계마다 음이 아닌 수를 준다. 확률은 `P_w(w₀) = ⟦e⟧(w₀) / Σ⟦e⟧`다. 확률은 초기 세계에만 둔다. 뒤에 드러나는 충격도 확률을 주려면 초기 세계의 셀로 선언하고 나중에 드러나게 한다.
- **평가값.** `value v = e`는 이력마다 값을 주는 이름이다. 지급액, 잔액, 성공 여부를 수로 바꾼 것 등 보호하거나 비교할 양을 적는다.

완결 이력의 집합을 `Out(M)`, 초기 세계 w₀에서 시작한 것을 `Out_{w₀}(M)`이라 한다. 통제 집합 C의 정책 π는 구성원마다 하나의 시야 기반 전략이다. `Out(M, π)`는 C의 모든 선택이 π를 따르는 완결 이력의 집합이며, W의 모든 초기 세계, 환경의 모든 가지, 모든 결제 후보, 나머지 구성원의 모든 선택을 포함한다. 필수 사건의 실패도 하나의 말단이다.

### 5.2 질의의 의미

| 질의 | 정의 | 판정의 근거 |
|---|---|---|
| `exists run: ψ` | ∃ρ ∈ Out(M), ⟦ψ⟧ρ | 참이면 이력 하나가 증인이다 |
| `forall run: ψ` | ∀ρ ∈ Out(M), ⟦ψ⟧ρ | 거짓이면 이력 하나가 반례다 |
| `can C: ψ` | ∃π ∈ Π(C), ∀ρ ∈ Out(M, π), ⟦ψ⟧ρ | 참이면 정책 하나가 증인이다. 거짓 판정에는 모든 정책을 깨는 이력들이 필요하다 |
| `max run: v`, `min run: v` | Out(M) 위에서 ⟦v⟧ρ의 최댓값, 최솟값 | 그 값을 내는 이력 |
| `expect w: min run: v` | Σ_{w₀} P_w(w₀) · min{⟦v⟧ρ ∣ ρ ∈ Out_{w₀}(M)} | 세계별 최솟값의 가중 평균이다 |
| `max policy C: Q` | Q의 Out(M)을 Out(M, π)로 바꾼 값의 π에 대한 최댓값 | 그 값을 내는 정책 |
| `region θ: Q` | {v ∈ Dom(θ) ∣ θ=v인 모형에서 Q가 참} | 집합을 돌려준다 |
| `prefer i at menu(m): a over b by v under w` | 아래 정의 | 정보집합별 두 평가값 |

- **수치 질의의 겹침.** `max policy`, `expect`, `max·min run`은 이 순서로만 겹친다. `expect w: min run: v`는 확률이 주어지지 않은 비결정성(환경의 가지, 결제 후보, 다른 구성원의 선택)을 세계마다 최악으로 해소한 보장 기댓값이다. 다른 구성원에게 선택이 있으면 이 값은 실제 최악 기댓값의 하한이다. `min` 대신 `max`를 쓰면 최선으로 해소한다.
- **정책과 정보.** 정책을 함께 골라도 구성원별 비공개 정보는 각자의 시야에 남는다. `∀세계 ∃행동`은 `∃정보 제약을 지키는 정책 ∀세계`보다 약하다. 이 차이가 7절의 결과를 가른다.
- **영역.** `region`은 공개 매개변수에 쓴다. 구성원에게 숨겨진 충격은 한 모형 안의 여러 세계에 두고 `can`으로 묻는다. 매개변수가 있는 모형에서 `region`으로 묶이지 않은 질의는 매개변수 배정마다의 결과 표를 돌려준다.
- **선호.** 메뉴 m이 표시된 지점들을 i의 시야로 묶은 것이 i의 정보집합이다. 정보집합 I와 행동 x에 대해, I의 지점을 지나고 거기서 x가 선택된 완결 이력들을 초기 세계별로 모아 v의 최솟값을 구하고, 이를 I에 닿는 세계들의 조건부 확률로 평균한 것이 평가값이다. `under`가 없으면 그 이력 전체의 최솟값이다. 모든 정보집합에서 a의 평가값이 b의 평가값보다 크면 참이다. 같으면 어느 쪽도 참이 아니며 이것이 무차별이다. 메뉴가 어느 이력에서도 표시되지 않으면 거짓이고, 정보집합에 닿는 세계들의 확률 합이 0이면 `D-Weight`다. 선호와 실제 결정권은 따로 묻는다.

### 5.3 판정 값과 주의

- 결과는 참·거짓, 수, 집합, 또는 매개변수 배정별 표다. 역산 질의의 결과는 5.4에 적는다. 탐색이 한도에서 중단되어 필요한 증명도 반증도 얻지 못하면 `UNKNOWN`이다. `region`에서 검사를 마치지 못한 매개변수 값은 참인 값의 집합에 넣지 않고 `unknown {…}`으로 따로 적는다. 정적 오류와 2.4의 실행 오류는 `INVALID_MODEL`이다.
- 실패 말단에서는 `at(end, φ)`가 거짓이다. 그래서 `forall run: at(end, not G)`가 거짓이어도 `exists run: at(end, G)`가 참이라는 보장이 없다. 실패 이력이 둘 다를 거짓으로 만들 수 있다. 필수 사건의 실패를 이력에 남겨 공허한 보장을 막기 위한 설계다.
- 전 기간의 위반을 묻는다면 위반 시점에 `label`을 두어 `reach`로 묻거나, 발생 후 참으로 유지되는 기록 셀을 둔다.
- 특정 도달 상태에서 결제해가 없다는 결과를 전체 계약의 모든 상태로 넓히려면 그 범위에 대한 증명이 따로 필요하다. ‘최소 충돌 조항’이라는 표현은 그 조항 집합이 함께 성립하지 않고 어느 하나를 뺐을 때 나머지가 성립함을 따로 확인했을 때만 쓴다.

<a id="solve"></a>
### 5.4 역산: 미지수와 `solve`

질의는 정해진 모형에서 목표가 성립하는지를 답한다. 역산은 거꾸로, 목표가 성립하려면 무엇이 어떤 값이어야 하는지를 답한다. 대상은 둘이다. 하나는 값을 정하지 않고 둔 상수(초기 보유량, 초기값, 금액, 시각 같은 계약 조건)이고, 다른 하나는 프로그램의 한 지점에서의 상태다.

- **미지수.** `unknown x : τ in lo..hi`는 값이 정해지지 않은 공개 상수다. 타입과 범위의 규칙은 `param`과 같고, 식에서 `param`을 쓰는 모든 자리에 쓴다. `param`은 값마다 결과를 표로 내지만 미지수의 값은 `solve`가 찾는다.
- **역산 질의.** `solve s = 꼴 [at(ℓ)] [where c] : g`다. 목표 g는 불 식이고, 선언된 질의의 이름을 그 판정 값으로 쓴다. `where c`는 실행하기 전에 후보를 거르는 조건이다.
- **후보.** `at`이 없으면 후보는 미지수들의 값 조합이다. `at(ℓ)`이 있으면 여기에 지점 ℓ의 상태가 더해진다. 상태는 시계 `now`, 모든 셀, 모든 보유량, 모든 슬롯(값·기준 시각·받은 시각, 또는 없음)이며, 선언된 범위 안이고 불변식을 만족하는 것만 후보다. 범위를 선언하지 않는 수량 슬롯의 값은 −1,000,000,000..1,000,000,000에서 찾는다. 그 상태에 실제로 도달하는 이력이 있는지는 따지지 않는다. `assume`은 초기 세계의 조건이므로 지점의 상태를 거르지 않는다.
- **사례.** 후보 하나를 넣은 모형이 사례다. `at`이 없으면 미지수에 그 값을 넣은 모형이다. `at(ℓ)`이 있으면 그 상태를 하나뿐인 초기 세계로 하고, ℓ 뒤에 남은 프로그램(ℓ이 적힌 블록의 나머지, 이어서 그 바깥 블록들의 나머지)을 실행하는 모형이다. 이 사례에서 `start`와 ℓ은 같은 지점이다. 지식은 그 상태에서 출발한 이력들 위에서 계산하므로, 출발 상태는 모두에게 알려진 것으로 본다.
- **해.** `where`가 참인 후보 가운데, 사례가 유효하고 그 사례에서 g가 참인 것이 해다. 사례가 유효하지 않으면(2.4의 `W-Range`·`W-Clock`과 `D-Init`·`D-Move`·`D-Weight`) 그 후보는 해가 아니다. 탐색이 한도에서 중단된 후보는 미판정이다.

| 꼴 | 결과 |
|---|---|
| `find` | 해 전체의 집합 |
| `any` | 가장 작은 해. 해가 없으면 `none` |
| `min e`, `max e` | 해 위에서 e의 최솟값·최댓값과 그 값을 내는 가장 작은 해. 해가 없으면 `none` |

- **순서.** ‘가장 작은’은 좌표를 차례로 견주는 사전식 순서다. 좌표의 차례는 미지수(선언 순서), `now`, 셀(선언 순서), 보유량(자산, 구성원 순), 슬롯(값, 기준 시각, 받은 시각)이다. 빈 값과 없는 슬롯이 가장 앞이고, 거짓이 참보다 앞이며, 열거값과 구성원은 선언 순서다.
- **표기.** 좌표가 하나인 `find`는 영역 질의와 같이 `{80..200}`으로, 수가 아닌 타입은 `{false, true}`처럼 값을 나열한다. 좌표가 여럿이면 조건의 묶음을 나열한다. `{[cap >= fee + 20], [mode=fast, when <= @2]}`에서 대괄호 하나는 그 안의 조건을 모두 만족하는 후보들이고, 전체는 그 합집합이다. 조건은 좌표 하나의 값(`paid=true`, `n=4..6`, `mode=slow|fast`, `hold!=empty`), 한쪽 끝(`cap >= 80`), 또는 좌표들의 일차식 사이의 비교(`due + bal(S, KRW) <= 3`)이며, 나눗셈의 몫은 `floor(x / 2)`·`ceil(x / 2)`로 적는다. 선언된 범위와 다른 조건이 이미 말해 주는 조건은 적지 않는다. 미판정 후보가 있으면 `unknown {…}`을 붙인다. `any`는 `cap=100`처럼 좌표마다 값을 적고, `min`·`max`는 `80 at cap=80`처럼 값 뒤에 해를 적는다(단위가 있으면 `2[KRW] at …`). 좌표의 이름은 미지수와 셀의 이름, `now`, `bal(i, x)`, `mem(i, k)`·`asof(i, k)`·`recv(i, k)`이고, 값은 수량을 단위 없이, 시각을 `@t`로, 빈 값과 없는 슬롯을 `empty`로 적는다. 답을 정하지 못하면 `UNKNOWN`이다. `any`는 가장 작은 해보다 앞선 미판정 후보가 있을 때, `min`·`max`는 미판정 후보가 하나라도 있을 때 그렇다.
- **일반 질의와 사례.** 미지수가 있는 모형의 일반 질의는 값이 정해져야 답할 수 있다. 그래서 역산 질의마다 대표 해 하나를 사례로 삼아 답한다. `any`와 `min`·`max`는 그 해, `find`는 가장 작은 해가 대표다. 결과는 `{need: true, short: false}`처럼 역산 질의의 이름별로 적고, 대표 해가 없으면 `none`이다. 매개변수가 있으면 대표 해가 있는 배정만으로 표나 영역을 만든다. 미지수가 없는 모형의 일반 질의는 이전과 같다.
- **정의와 계산.** 위의 정의는 후보를 하나씩 넣어 보는 것이다. 참조 구현과 Python 엔진은 정의대로 계산하며 후보가 200,000개를 넘으면 `UNKNOWN`을 낸다. [Rust 엔진](engine.md)의 솔버는 후보를 하나씩 넣는 대신 판정이 달라지는 경계를 찾아 같은 답을 내므로 범위가 넓어도 푼다. 좌표가 여럿인 `find`의 조건 묶음은 솔버의 표기이고, 정의대로 계산하는 엔진은 같은 집합을 해의 나열로 적는다. 이 한 가지 꼴만 글자가 다르며, 검산은 후보마다의 판정으로 대조한다.

<a id="library"></a>
## 6. 표준 계약

매매·대차·권리·콜은 언어 자체로 쓴 틀이다. 아래 블록이 모든 모델 앞에 놓인다. 틀의 셀은 그 인스턴스의 사건만 쓰고 당사자가 관찰한다.

```fean-prelude
enum Life       { absent, active, settled }
enum Deal       { open, agreed, delivered }
enum RightState { unset, live, used, lapsed }

// 매매. 체결(agree)과 결제(settle)를 나누거나 한 번에(spot) 커밋한다.
contract Sale(seller : Actor, buyer : Actor, x : Asset, q : Qty[x], y : Asset, p : Qty[y]) {
  cell stage : Deal init open observe seller, buyer
  event agree by seller, buyer { require stage == open  stage := agreed }
  event settle by seller {
    auth stage == agreed
    move seller -> buyer : q
    move buyer -> seller : p
    stage := delivered
  }
  event spot by seller, buyer {
    require stage == open
    move seller -> buyer : q
    move buyer -> seller : p
    stage := delivered
  }
}

// 대차. 기존 자금을 빌려주고 약정 채권을 세운다.
contract Loan(lender : Actor, borrower : Actor, x : Asset, principal : Qty[x], face : Qty[x], due : Time) {
  cell life   : Life              init absent observe lender, borrower
  cell remain : Qty[x] in 0..face init 0      observe lender, borrower
  cell holder : Actor             init lender observe lender, borrower
  cell late   : Qty[x] in 0..face init 0      observe lender, borrower
  cell waived : Qty[x] in 0..face init 0      observe lender, borrower
  event originate by lender {
    require life == absent
    require 0[x] < principal and 0[x] < face
    move lender -> borrower : principal
    remain := face
    life := active
  }
  event repay(n : Qty[x]) by borrower {
    require life == active
    require 0[x] < n and n <= remain
    move borrower -> holder : n
    remain -= n
    life := if n == remain then settled else active
  }
  event assign(to : Actor) by holder { require life == active  holder := to }
  event forgive(n : Qty[x]) by holder {
    require life == active
    require 0[x] < n and n <= remain
    remain -= n
    waived += n
    life := if n == remain then settled else active
  }
  event close by holder { require life == active  require due <= now  late := remain }
}

// 권리. 보유자가 기간 안에 상대의 추가 동의 없이 쓴다.
contract Right(holder : Actor, writer : Actor, from : Time, to : Time) {
  cell state : RightState init unset observe holder, writer
  event grant by writer { require state == unset  state := live }
  event use by holder {
    require state == live
    require from <= now and now <= to
    state := used
  }
  event lapse by writer { require state == live  require to < now  state := lapsed }
}

// 실물 콜. 프리미엄 지급과 권리 설정을 한 묶음으로, 행사와 약정 매매를 한 묶음으로 커밋한다.
contract Call(holder : Actor, writer : Actor, u : Asset, q : Qty[u], k : Asset, strike : Qty[k],
              premium : Qty[k], from : Time, to : Time) {
  instance right = Right(holder, writer, from, to)
  instance deal  = Sale(writer, holder, u, q, k, strike)
  event pay by holder { move holder -> writer : premium }
  batch open     = { pay, right.grant }
  batch exercise = { right.use, deal.spot }
  proc decide() {
    choose holder menu callMenu {
      exercise => { run batch exercise }
      pass     => { }
    }
  }
}
```

| 틀 | 전이와 조건 |
|---|---|
| `Sale` | 대상 x의 q와 대가 y의 p를 반대로 옮긴다. 인도와 대금이 연계된 결제는 두 `move`를 한 사건에 둔다. 체결일과 결제일이 다르면 `agree`로 의무를 세우고 `settle`에서 실제 인도와 지급을 커밋한다. `settle`이 매수인의 보유량을 줄이는 근거는 앞선 체결이다 |
| `Loan` | 아직 쓰지 않은 채권에 채권자·채무자·지급 자산·기일을 준다. `originate`는 원금 이동과 약정 잔액 설정을 한 사건으로 묶는다. 원금과 약정액은 양수이고 같아도 된다. `repay`는 활성 채권에서 `0 < n ≤ remain`일 때 현재 채권자에게 지급하고 잔액을 줄이며 0이면 `settled`다. `assign`은 채권자를 바꾸므로 이후 상환의 수취인이 바뀐다. `forgive`는 지급 없이 잔액을 줄이고 그 양을 `waived`에 남긴다. `close`는 기일이 된 뒤 남은 잔액을 연체액 `late`에 적는다 |
| `Right` | 보유자가 기간 안에 쓸 수 있는 권한이다. 무엇을 발동하는지는 `use`를 다른 사건과 한 묶음으로 커밋해 정한다. 양도 가능한 권리는 보유자를 셀에 두는 별도의 틀로 쓴다 |
| `Call` | 틀 안에 `Right`와 `Sale`을 둔다. 프리미엄 지급과 권리 설정 뒤, 보유자가 행사하면 약정 매매가 같은 커밋에서 실행되고 아니면 만료된다 |

은행 대출에 따른 예금 발행, 실물 폐기, 법에 의한 의무 변경은 `auth`를 적은 생성·소멸·변경 사건으로 선언한다. 보유량을 바꾸는 효과는 `move`와 `auth`가 있는 증감뿐이므로 자산별로 **기말 수량 = 기초 수량 + 유입 − 유출 + 생성 − 소멸**이 성립한다. 시가 변동은 가격 셀에 기록하고 수량과 섞지 않는다.

**대차의 수명주기.** 원금 100원, 5% 이자를 붙인 약정액 105원이다. 60원을 갚고, 채권이 양도된 뒤 40원을 갚고, 마지막 5원은 재원이 없어 거절된다.

```fean
model LoanLifecycle {
  actor Lender, Debtor, Buyer
  asset KRW in 0..100 init Lender = 100
  time 0..4

  def withInterest(p : Qty[KRW], bp : Num) : Qty[KRW] = floor(p * (10000 + bp) / 10000)

  instance c = Loan(Lender, Debtor, KRW, 100[KRW], withInterest(100[KRW], 500), @3)

  program {
    at @0 { run c.originate }
    at @1 { run c.repay(60[KRW])  label first }
    at @2 { run c.assign(Buyer)  run c.repay(40[KRW])  label second }
    at @3 { attempt c.repay(5[KRW]) { } else { label refused }  run c.close }
  }

  query afterFirst  = forall run: at(first, bal(Lender, KRW) == 60[KRW] and c.remain == 45[KRW])  // => true
  query afterSecond = forall run: at(second, bal(Buyer, KRW) == 40[KRW] and c.remain == 5[KRW])   // => true
  query lastRefused = forall run: reach(refused)                                                  // => true
  query overdue     = max run: at(end, c.late) ?? 0[KRW]                                          // => 5[KRW]
  query debtorKnows = forall run: at(end, K(Debtor, c.holder == Buyer))                           // => true
}
```

**콜옵션.** 프리미엄 5원, 행사가 100원, 1주다. 행사하면 발행자가 105원을 갖고 보유자가 주식을 갖는다. 포기하면 보유자에게 100원이, 발행자에게 주식이 남는다.

```fean
model CallExample {
  actor H, W
  asset KRW   in 0..105 init H = 105
  asset share in 0..1   init W = 1
  time 0..3

  instance call = Call(H, W, share, 1[share], KRW, 100[KRW], 5[KRW], @1, @2)

  program {
    at @0 { run batch call.open }
    at @1 { do call.decide() }
    at @3 { attempt call.right.lapse { } }
  }

  query exercised   = exists run: at(end, bal(W, KRW) == 105[KRW] and bal(H, share) == 1[share])   // => true
  query passed      = exists run: at(end, bal(H, KRW) == 100[KRW] and call.right.state == lapsed)  // => true
  query premiumKept = forall run: at(end, bal(W, KRW) >= 5[KRW])                                   // => true
  query holderCan   = can {H}: at(end, bal(H, share) == 1[share])                                  // => true
  query writerCan   = can {W}: at(end, bal(W, share) == 1[share])                                  // => false
}
```

<a id="example"></a>
## 7. 완결 사례 · 누가 어느 시점에 80원 상환을 보장할 수 있는가

구성원은 채무자 D, 채권자 L, 청구권 발행자 I, 감사·인증기관 J다. 초기 세계는 `regime`이 호황인 세계와 불황인 세계 둘이다. D는 시각 2에 A 또는 B를 고른다. 호황에는 A만, 불황에는 B만 100원을 지급한다. J의 최초 관찰은 정확하다고 가정한다. 공개 매개변수 `early`는 통지 일정이다. 참이면 J가 시각 1에, 거짓이면 시각 3에 D에게 통지한다.

```fean
model Repay80 {
  actor D, L, I, J
  asset KRW in 0..100 init I = 100
  enum Regime { boom, slump }
  enum Pick   { none, A, B }
  time 0..4
  param early : Bool

  cell regime   : Regime            init in { boom, slump }
  cell selected : Pick              init none   write elect
  cell proof    : Regime?           init empty  write certify  observe I
  cell remain   : Qty[KRW] in 0..80 init 80     write collect  observe D, L
  cell life     : Life              init active write collect  observe D, L

  slot report    : Regime for D, J
  slot ownChoice : Pick   for D

  event audit by J { tell J.report := regime asof @0 }
  event notice(when : Bool) by J {
    require early == when
    given r = mem(J, report)
    given u = asof(J, report)
    tell D.report := r asof u
  }
  event elect(a : Pick) by D {
    require selected == none
    selected := a
    tell D.ownChoice := a asof now
  }
  event certify by J { given r = mem(J, report)  proof := r }
  event award by I {
    require selected != none
    given e = proof
    let n = if (selected == A and e == boom) or (selected == B and e == slump) then 100[KRW] else 0[KRW]
    move I -> D : n
  }
  event collect by D {
    require life == active
    let b = min(bal(D, KRW), remain)
    move D -> L : b
    remain -= b
    life := if b == remain then settled else active
  }

  program {
    at @0 { run audit }
    at @1 { attempt notice(true) { } }
    at @2 { choose D menu claimMenu { A => { run elect(A) }  B => { run elect(B) } } }
    at @3 { attempt notice(false) { }  run certify  run award  label funded }
    at @4 { run collect }
  }

  weight prior = 1
  value cash = at(funded, bal(D, KRW)) ?? 0[KRW]
  value paid = if at(end, remain == 0[KRW]) then 1 else 0

  query possible = POSSIBLE(remain == 0[KRW])               // => {early=false: true, early=true: true}
  query always   = ALL(remain == 0[KRW])                    // => {early=false: false, early=true: false}
  query canD     = region early: CAN({D}, remain == 0[KRW]) // => {true}
  query knowsD   = region early: forall run: at(menu(claimMenu), K(D, regime == boom) or K(D, regime == slump))  // => {true}
  query knowsJ   = region early: forall run: at(menu(claimMenu), K(J, regime == boom) or K(J, regime == slump))  // => {false, true}
  query knowsL   = region early: forall run: at(menu(claimMenu), K(L, regime == boom) or K(L, regime == slump))  // => {}
  query laterD   = forall run: at(funded, K(D, regime == boom) or K(D, regime == slump))  // => {early=false: true, early=true: true}
  query creditor = forall run: at(end, K(L, remain == 0[KRW]) or K(L, remain != 0[KRW]))  // => {early=false: true, early=true: true}
  query worst    = max policy {D}: min run: cash               // => {early=false: 0[KRW], early=true: 100[KRW]}
  query chance   = max policy {D}: expect prior: min run: paid // => {early=false: 1/2, early=true: 1}
}
```

### 시각 2, 선택 직전의 인지 단면

`menu(claimMenu)`은 메뉴가 D에게 표시되고 D가 아직 고르기 전의 지점이다. 증거 수리는 시각 3이므로 이 지점에서 `proof`는 비어 있다.

| 통지 일정 | D가 경기를 아는가 | J가 아는가 | L이 아는가 | D의 report 슬롯 |
|---|---|---|---|---|
| `early = true`, 시각 1 수신 | 안다 | 안다 | 모른다 | (경기, 기준시각 0, 수신시각 1) |
| `early = false`, 시각 3 수신 | 모른다 | 안다 | 모른다 | 빈 값 |

지연 일정에서도 시각 3의 `funded` 지점에서는 D가 경기를 안다(`laterD`). 그 지점의 지식은 참이지만 시각 2의 선택은 이미 이력에 확정되어 있다. 채권자 L은 통지를 받지 않아도 관찰하는 잔액으로 상환 여부를 안다(`creditor`).

### 질의 결과

| 질의 또는 수량 | `early = true` | `early = false` |
|---|---:|---:|
| `possible` | 참 | 참 |
| `always`, D가 잘못 고르는 경우도 허용 | 거짓 | 거짓 |
| `canD` | **참** | **거짓** |
| `worst`, 최선 정책의 최악 가용 현금 | **100원** | **0원** |
| `chance`, 두 세계가 각각 1/2일 때 최선 정책의 상환 성공 확률 | 1 | 1/2 |

조기 통지의 증인 정책은 ‘report가 호황이면 A, 불황이면 B’다. 지연 통지에서 D가 A를 고르는 정책은 불황에서, B를 고르는 정책은 호황에서 실패한다. 선택 시점의 D에게 두 세계는 같은 시야이므로 두 실패를 함께 피하는 결정론적 정책이 없다. 정보와 독립인 무작위 선택을 허용해도 이 예제의 확률 상한과 전 세계 보장은 달라지지 않는다. **100원은 상환 전 재원이고 실제로 갚는 최대 채무액은 80원**이다.

## 8. 영역, 선호, 역산의 사례

**강건 영역.** 행사가 100원인 현금결제형 콜과 풋 각 1단위를 보유한다. 프리미엄은 지급 완료, 다른 재원은 없고 10원 채무가 옵션 정산 뒤 만기다. 선언 가격 S가 0~200의 정수일 때 결제액은 |S−100|원이다. 상환 가능 영역은 두 구간으로 갈라진다. 보호 명제와 비교 단위를 바꾸면 영역도 바뀐다.

```fean
model Straddle {
  actor H, W, C
  asset KRW in 0..200 init W = 200
  time 0..1
  param S : Num in 0..200
  cell debt : Qty[KRW] in 0..10 init 10 write repay observe H, C

  def payoff() : Qty[KRW] = (if S <= 100 then 100 - S else S - 100) * 1[KRW]

  event settle by W { move W -> H : payoff() }
  event repay by H {
    let b = min(bal(H, KRW), debt)
    move H -> C : b
    debt -= b
  }

  program { run settle  run repay }

  query safe = region S: forall run: at(end, debt == 0[KRW])   // => {0..90, 110..200}
}
```

**유인.** 지금 청산하면 75원이 회수되어 선순위 50원, 후순위 25원을 받는다. 유예하면 확률 pn/60으로 80원이 회수되어 50원과 30원을, 나머지 확률로 0원을 받는다. 위험중립이고 추가 비용과 시간할인은 0이다. 선순위는 pn<60에서 청산을, 후순위는 pn>50에서 유예를 선호하고 pn=50에서 후순위는 무차별이다. 두 선호가 엇갈리는 구간은 51~59, 곧 5/6 < p < 1이다.

```fean
model Workout {
  actor Senior, Junior, Firm
  asset KRW in 0..80
  time 0..1
  param pn : Num in 0..60
  cell good : Bool init in { true, false }

  event liquidate by Firm { auth true  bal(Firm, KRW) += 75[KRW] }
  event recover by Firm { auth true  bal(Firm, KRW) += if good then 80[KRW] else 0[KRW] }
  event waterfall by Firm {
    let s = min(50[KRW], bal(Firm, KRW))
    let j = min(30[KRW], bal(Firm, KRW) - s)
    move Firm -> Senior : s
    move Firm -> Junior : j
  }

  program {
    choose Firm menu workout {
      liquidate => { run liquidate }
      defer     => { at @1 { run recover } }
    }
    run waterfall
  }

  weight chance = if good then pn else 60 - pn
  value toSenior = at(end, bal(Senior, KRW)) ?? 0[KRW]
  value toJunior = at(end, bal(Junior, KRW)) ?? 0[KRW]

  query seniorLiquidates = region pn: prefer Senior at menu(workout): liquidate over defer by toSenior under chance  // => {0..59}
  query juniorDefers     = region pn: prefer Junior at menu(workout): defer over liquidate by toJunior under chance  // => {51..60}
  query juniorLiquidates = region pn: prefer Junior at menu(workout): liquidate over defer by toJunior under chance  // => {0..49}
}
```

일반적으로 기회비용을 평가하려면 서로 배타적인 실행 가능 대안과 주체의 평가값이 필요하다. 확률·위험 선호·상대의 반응이 필요하면 `weight`와 `value`로 선언한다.

**필요 자본의 역산.** 발행자 I가 채무자 D에게 가진 돈을 모두 넘기고, D는 시각 1에 채권자 L에게 80원을 갚는다. I의 초기 자금 `cap`을 미지수로 둔다. 상환이 보장되는 최소 자금은 80원이고, 그보다 1원이 적으면 보장이 깨진다. 일반 질의는 역산 질의의 대표 해마다 답한다.

```fean
model Capital {
  actor I, D, L
  unknown cap : Num in 0..200
  asset KRW in 0..200 init I = cap
  time 0..1
  cell remain : Qty[KRW] in 0..80 init 80 write collect observe D, L

  event award by I { move I -> D : bal(I, KRW) }
  event collect by D {
    let b = min(bal(D, KRW), remain)
    move D -> L : b
    remain -= b
  }

  program { run award  at @1 { run collect } }

  query repaid = forall run: at(end, remain == 0[KRW])       // => {need: true, enough: true, spare: true, short: false}
  query left   = max run: at(end, bal(D, KRW)) ?? 0[KRW]     // => {need: 0[KRW], enough: 0[KRW], spare: 20[KRW], short: 0[KRW]}

  solve need   = min cap: repaid                             // => 80 at cap=80
  solve enough = find: repaid                                // => {80..200}
  solve spare  = any: repaid and left >= 20[KRW]             // => cap=100
  solve short  = max cap where cap <= 150: not repaid        // => 79 at cap=79
}
```

**지점의 상태.** 매수인 B가 시각 1에 대금 `due`를 매도인 S에게 치른다. `ready` 지점에서 어떤 상태여야 결제가 보장되는가를 묻는다. 미지수가 없으므로 일반 질의 `settled`는 선언된 초기 세계에서 답하고, 역산 질의는 `ready`의 모든 상태를 후보로 삼는다. 결제되는 상태는 두 묶음이다. 이미 치렀고 다시 치를 돈이 없는 상태와, B에게 대금이 있고 S가 그것을 받을 수 있는 상태다. S의 보유량이 한도 3원에 닿아 있으면 1원도 받을 수 없어 해가 없다.

```fean
model Escrow {
  actor B, S
  asset KRW in 0..3 init B = 3
  time 0..1
  cell due  : Qty[KRW] in 0..3 init 2 write pay
  cell paid : Bool init false write pay

  event pay by B {
    require bal(B, KRW) >= due
    move B -> S : due
    paid := true
  }

  program { label ready  at @1 { attempt pay { } } }

  query settled = forall run: at(end, paid)                                              // => true

  solve pre   = find at(ready): settled                                                  // => {[due <= bal(B, KRW), due + bal(S, KRW) <= 3], [paid=true, due >= bal(B, KRW) + 1]}
  solve least = any at(ready) where not paid: settled                                    // => now=@0, due=0, paid=false, bal(B, KRW)=0, bal(S, KRW)=0
  solve cheap = min bal(B, KRW) at(ready) where due == 2[KRW] and not paid: settled      // => 2[KRW] at now=@0, due=2, paid=false, bal(B, KRW)=2, bal(S, KRW)=0
  solve full  = any at(ready) where bal(S, KRW) == 3[KRW] and due == 1[KRW] and not paid: settled  // => none
}
```

## 9. 검증의 범위

이 언어를 실제로 돌리는 것은 [엔진](engine.md)이다. 엔진은 모델 글을 실행 전에 중간표현으로 컴파일하고 그것을 실행해 질의 결과를 낸다.

[참조 구현](../tests/fean_ref/__init__.py)은 이 문서의 문법으로 모델을 읽고 2절의 규칙으로 검사하며 3~5절의 의미로 모든 이력을 열거한다. 역산 질의는 5.4의 정의대로 후보를 하나씩 넣어 답한다. [v0.2 검산](../tests/test_fean_v02.py)은 이 문서의 모든 모델 블록을 실행해 `// =>` 주석과 대조하고, 규칙마다 그것을 어긴 모델이 거절되는지 확인한다. [v0.1 검산](../tests/test_financial_language.py)은 같은 사례들의 산술을 독립적으로 열거한 것으로, 그대로 유지한다.

검산이 뒷받침하는 범위는 명시한 유한 입력까지다. 완전검사 프로필은 유한한 초기 세계·수량 범위·시각·선택지를 쓰며 프로그램에 반복이 없다. 평가의 전체성과 전략 실현은 2.3과 4.3에 근거를 적은 주장이고 기계로 증명한 것은 아니다. 다른 범위로 일반화할 때에는 그 범위에서 따로 검증해야 한다. [유한상태 모형 검증](https://www-users.cse.umn.edu/~heimdahl/csci8801-fall06/readings/model-checking-clarke.pdf) · [시나리오별 정보 제약](https://webdoc.sub.gwdg.de/ebook/e/2003/zib_2/reports/ZR-01-24.pdf)

## 변경 이력

- 2026-11-22 · v0.1 초안. 구성원별 인지, 원자적 결제, 정보에 맞는 전략과 80원 상환 사례를 정의했다.
- 2026-11-22 · 정보 제약이 적용되는 행동 인수와 판정 범위를 명료화했다.
- 2026-11-22 · v0.2. 문장형 표면 문법과 타입·층·구조 규칙, 틀과 매개변수, 관찰 선언과 관찰 지점, 총액 기본 결제와 실패 분류, 선언하는 지점·가중치·평가값과 겹쳐 쓰는 질의, 언어로 쓴 표준 계약을 정의했다.
- 2026-11-23 · v0.3. 미지수 `unknown`과 역산 질의 `solve`, 목표층 V와 X, 규칙 `Q-Solve`, 사례에서의 일반 질의, 예제 `Capital`과 `Escrow`를 더했다.
- 2026-11-23 · T-Name과 T-Call에 `def`의 몸과 질의 이름의 관계를 밝혔다. 질의와 미지수가 같은 이름을 쓸 때 `def`의 몸에서는 미지수다.
- 2026-11-24 · 저장소에 싣지 않는 결정 기록으로 가는 링크를 뺐다. 내용은 바꾸지 않았다.
- 2026-11-24 · 수식을 GitHub이 그리는 `math` 블록으로 옮기고 이름의 표기를 `\mathrm`·`\mathtt`로 바꿨다. 식의 뜻은 바꾸지 않았다.
