# ADR-0001: worklist — REST como transporte por defecto del puerto

**Estado:** Aceptado **Fecha:** 2026-09-07

Ejecuta `ACC-315`. Primera decisión registrada de esta capa: hasta ahora el razonamiento del puerto vivía entero en [`concepts/sync.md`](../../../concepts/sync.md) § "El puerto son las operaciones, no los comandos", que dice **qué** hace cada operación; ésta dice **con qué se hacen las que vengan**.

---

## Contexto

El puerto tiene dos transportes, `acli` y `jira-cli`, y una tabla —`Op::transport`— que reparte cada operación entre ellos. La tabla existe porque **cada CLI no puede algo**: `jira-cli` se lleva `SetParent`, `AddToSprint` y `SprintList` por lo que `acli` no acepta al editar; `acli` se queda con `ParentOf` por lo que `jira-cli` no lee. Cada fila es el agujero de alguien.

`ACC-276` agregó dos operaciones de estado —`Transition` y `TransitionsOf`— y las asignó las dos a `acli`, escribiéndolas *"con la misma forma que el resto de las escrituras"* y marcándolas como **no medidas contra el board real**. Al medirlas, el 2026-09-07:

| | listar transiciones | mover de estado | poner la resolución |
| --- | --- | --- | --- |
| `acli` | **no existe el subcomando** — `acli jira workitem` tiene `transition`, no `transitions` | sí | **no tiene el flag**, ni en `transition` ni en `edit` |
| `jira-cli` | **no existe el subcomando** — `jira issue move` sin estado abre un selector interactivo | sí | tiene el flag |
| REST | sí, y devuelve los ids | sí, por id | sí |

**`TransitionsOf` no tiene transporte posible entre los dos que hay.** Y no es que uno lo haga peor: `acli` no tiene el comando, y la única forma que `jira-cli` ofrece es un selector por consola — que `board.rs` ya descarta por escrito, porque *"un hook no tiene terminal donde contestar: un argumento de menos no es un error, es un proceso colgado"*.

Hay un segundo hecho, más chico y más filoso: **el nombre de una transición no es el del status al que lleva.** En este board la transición se llama `Listo` y deja el ítem en `Finalizada`. `acli` pide `--status`, `jira issue move` pide un `STATE`, y ninguno documenta cuál de los dos nombres espera. Cualquiera de los dos anda **por casualidad** en un board donde los nombres coinciden.

---

## Decisión

### 1. Una operación nueva del puerto se implementa contra REST

De acá en adelante, agregar una fila a la tabla del reparto es agregarla con `Transport::Api`. No hace falta justificar la elección; hace falta justificar **no** elegirla.

**Y es asimétrico a propósito.** Los dos CLIs entraron por lo que sabían hacer, así que cada operación nueva reabría la pregunta *"¿cuál de los dos puede ésta?"* y la contestaba mirando dos manuales. REST no tiene agujeros conocidos, así que puede ser el default sin que la pregunta vuelva. **La tabla deja de crecer por descarte.**

### 2. Lo que ya está medido no se muda

Las nueve filas que hoy andan por `acli` o `jira-cli` **se quedan donde están**. Esto no es una migración.

Mover una fila que funciona cuesta una medición nueva —la única forma de saber que el reemplazo anda es correrlo contra el board— y no compra nada que hoy falte. Una fila se muda cuando tenga **su propia** razón: que el CLI no pueda algo que empezó a hacer falta, o que su forma de fallar cueste más que reescribirla.

**El costo que sí se acepta es tener tres.** § "Tres transportes, dos formas de mentir, una sola respuesta" ya dice que normalizar los modos de falla es lo que evita que un transporte más multiplique lo que hay que saber río arriba. Un tercero paga ese costo una vez.

### 3. Las dos operaciones de estado nacen en REST

Son la excepción que la decisión 2 admite, y por la razón que la 2 pide: `TransitionsOf` **no se puede** implementar con ninguno de los dos CLIs, y `Transition` la necesita para conseguir el id.

**No se está mudando nada**: ninguna de las dos corrió jamás contra un board, así que no hay medición que se pierda ni comportamiento del que algo dependa. En los términos de la decisión 1, son operaciones nuevas que todavía no habían elegido bien.

### 4. La transición se pide por id, y el id sale de listar

```
listar las transiciones del issue
  → buscar la que tiene como destino el status que el mapeo pide
  → pedir esa transición por su id
```

El [mapeo de la instalación](../../../concepts/states.md#el-vocabulario-está-en-git-el-mapeo-en-la-instalación) sigue escrito con **nombres de status**, que es lo que un humano puede leer del board. El id lo resuelve el código en el momento, contra el workflow vigente.

Es la misma preferencia que el resto del sistema: [una tabla de transiciones acá sería una copia que empieza desactualizada](../../../concepts/states.md#worklist-no-tiene-reglas-de-transición-el-proveedor-sí). Un id cacheado sería lo mismo un nivel más abajo.

**Y esto invierte el orden de `TransitionsOf`.** Estaba escrita como *"se pide sólo tras un rechazo"*, para no gastar una llamada en un dato que casi nunca se usa. Ahora precede al intento, y de ahí salen dos consecuencias que van juntas:

| | |
| --- | --- |
| **el costo** | una llamada por ítem **que cambia de estado** — no por ítem, y no por rechazo |
| **lo que se gana** | un rechazo por regla **siempre** puede decir cuáles sí. Antes podía quedarse en *"no se pudieron listar"* |

La degradación no se vuelve improbable: **desaparece**. Si el listado falla no hay id, así que no hay transición que intentar, y el fracaso es *"no se pudo preguntar"* antes de tocar nada.

### 5. La credencial es la misma; lo que se agrega es el email

REST autentica con `Basic base64(email:token)`. El token ya llega por `JIRA_API_TOKEN` y el hook lo hereda del entorno de quien empuja — no cambia nada de eso.

**El email no es un secreto, y por eso no viaja con el token.** Es un dato de la instalación, del mismo lado que la URL base y el id del board, que ya viajan como argumentos del hook.

Y **no se lee del config de `jira-cli`**, aunque esté ahí. Atar el tercer transporte a la instalación del segundo es exactamente lo que la decisión 1 quiere dejar de hacer: volvería a `jira-cli` un requisito de REST, y entonces la fila que se escriba mañana contra REST seguiría dependiendo de un binario que no usa.

---

## Consecuencias

**El puerto pasa a tener un cliente HTTP propio**, que es lo que las dos elecciones anteriores habían evitado. `sync.md` decía que la elección del segundo transporte *"fue entre escribir un cliente HTTP y administrar un binario más"*, y que la membresía de sprint —de lote— inclinaba la balanza. Ese argumento sigue en pie **para esa fila**; lo que cambió es que apareció una operación donde no hay binario que administrar.

**La columna de REST en la tabla de modos de falla no está medida.** Las de los CLIs salieron de correrlos y de leerles el código. Ésta sale de lo que la API documenta, y vale lo que valía la de `jira-cli` antes de que alguien la mirara: está escrita para que la primera corrida tenga contra qué compararse.

**Lo que este ADR no arregla:** que en este board `dropped` no se pueda distinguir de `done`. Ninguna de las tres transiciones del workflow de ACC acepta campos, y `resolution` no está en el `editmeta`, así que la resolución no se puede escribir **por ningún transporte**. Es un dato de la instalación y lo dice el mapeo, no el puerto — `Destino::Completo` con su resolución sigue siendo la forma correcta para una instalación que sí pueda.
