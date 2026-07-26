import { Hero } from "./components/Hero";
import { Compare } from "./components/Compare";
import { Workflow } from "./components/Workflow";
import { Docs } from "./components/Docs";
import { Closing } from "./components/Closing";
import { Footer } from "./components/Footer";

function App() {
  return (
    <div className="page">
      <Hero />
      <Compare />
      <Workflow />
      <Docs />
      <Closing />
      <Footer />
    </div>
  );
}

export default App;
