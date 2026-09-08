// Dumps decompiled C for every function and a symbol/string index. Args: outDir
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import java.io.*;
public class ExportDecomp extends GhidraScript {
  public void run() throws Exception {
    String[] a = getScriptArgs(); String out = a.length>0?a[0]:"/tmp/ghidra_out";
    String base = out + "/" + currentProgram.getName();
    new File(base).mkdirs();
    DecompInterface d = new DecompInterface(); d.openProgram(currentProgram);
    d.setOptions(new DecompileOptions());
    PrintWriter all = new PrintWriter(new FileWriter(base + "/all_functions.c"));
    PrintWriter idx = new PrintWriter(new FileWriter(base + "/functions.txt"));
    FunctionIterator it = currentProgram.getFunctionManager().getFunctions(true);
    int n=0;
    while (it.hasNext() && !monitor.isCancelled()) {
      Function f = it.next();
      if (f.isThunk() || f.isExternal()) continue;
      DecompileResults r = d.decompileFunction(f, 60, monitor);
      idx.println(f.getEntryPoint() + " " + f.getName() + " size=" + f.getBody().getNumAddresses());
      all.println("// ===== " + f.getName() + " @ " + f.getEntryPoint());
      if (r != null && r.decompileCompleted()) all.println(r.getDecompiledFunction().getC());
      else all.println("// decompile failed");
      n++;
    }
    all.close(); idx.close();
    PrintWriter sw = new PrintWriter(new FileWriter(base + "/strings_xref.txt"));
    for (ghidra.program.model.listing.Data dt : currentProgram.getListing().getDefinedData(true)) {
      if (dt.hasStringValue()) {
        StringBuilder refs = new StringBuilder();
        for (Reference ref : getReferencesTo(dt.getAddress())) {
          Function ff = getFunctionContaining(ref.getFromAddress());
          refs.append(ff!=null?ff.getName():ref.getFromAddress().toString()).append(",");
        }
        sw.println(dt.getAddress() + "\t" + dt.getDefaultValueRepresentation() + "\t" + refs);
      }
    }
    sw.close();
    println("Exported " + n + " functions to " + base);
  }
}
